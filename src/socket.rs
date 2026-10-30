//! Sockets Unix (`AF_UNIX`): comunicação entre processos da mesma máquina.
//!
//! Um pipe liga dois processos aparentados (quem cria o pipe passa as pontas
//! para os filhos). Um socket Unix tem um *endereço* — um caminho no sistema
//! de arquivos, como `/tmp/servidor.sock` —, e qualquer processo que tenha
//! permissão de escrita nesse arquivo pode se conectar a ele. É assim que o X,
//! o D-Bus e o syslog (`/dev/log`) recebem clientes.
//!
//! ```text
//!   servidor                          cliente
//!   s = socket(AF_UNIX, SOCK_STREAM)
//!   bind(s, "/tmp/x.sock")            c = socket(AF_UNIX, SOCK_STREAM)
//!   listen(s, 5)
//!   a = accept(s)  ←───────────────── connect(c, "/tmp/x.sock")
//!   read(a) / write(a) ←────────────→ write(c) / read(c)
//! ```
//!
//! Dois tipos:
//! - `SOCK_STREAM`: uma conexão, um fluxo de bytes nos dois sentidos. Por
//!   dentro, são dois pipes (um para cada lado).
//! - `SOCK_DGRAM`: mensagens avulsas, cada uma inteira, endereçadas a um
//!   socket ligado a um caminho (sem conexão).
//!
//! Endereços que começam com um byte zero são "abstratos" (uma extensão do
//! Linux): não aparecem no sistema de arquivos e somem com o socket.

use crate::arquivo::{self, Arquivo, Pipe, Tipo, POLLHUP, POLLIN, POLLOUT};
use crate::erro::*;
use crate::processo;
use crate::spin::Mutex;
use crate::syscall::{arquivo, cred, escrever_usuario, ler_usuario, novo_descritor, pai_gravavel, Resultado};
use crate::vfs::{self, Meta};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

const AF_UNIX: u64 = 1;
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;

const MSG_DONTWAIT: u64 = 0x40;
const MSG_NOSIGNAL: u64 = 0x4000;

/// Quantas mensagens um socket de datagramas guarda sem ninguém ler.
const MAX_DATAGRAMAS: usize = 128;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Endereco {
    Nenhum,
    Caminho(String),
    Abstrato(Vec<u8>),
}

/// Como um endereço ligado é achado: pelo nó do arquivo (que pode ter sido
/// renomeado, ou alcançado por outro caminho) ou pelo nome abstrato.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Chave {
    No(u64),
    Abstrato(Vec<u8>),
}

static LIGADOS: Mutex<BTreeMap<Chave, Weak<Socket>>> = Mutex::new(BTreeMap::new());

/// (pid, uid, gid): o SO_PEERCRED, quem está do outro lado.
type Credenciais = (u32, u32, u32);

enum Estado {
    Novo,
    Ouvindo { fila: VecDeque<Arc<Socket>>, maximo: usize },
    Conectado { entrada: Arc<Pipe>, saida: Arc<Pipe>, par: Endereco, cred_par: Credenciais, leitura: bool, escrita: bool },
    /// Datagramas com destino padrão (`connect` num SOCK_DGRAM, ou socketpair).
    Destino { para: Weak<Socket>, endereco: Endereco },
}

pub struct Socket {
    pub tipo: u32,
    estado: Mutex<Estado>,
    endereco: Mutex<Endereco>,
    chave: Mutex<Option<Chave>>,
    /// SOCK_DGRAM: mensagens que chegaram (dados, endereço de quem mandou).
    caixa: Mutex<VecDeque<(Vec<u8>, Endereco)>>,
    credenciais: Credenciais,
}

fn minhas_credenciais() -> Credenciais {
    let p = processo::atual();
    (p.tgid, p.euid, p.egid)
}

impl Socket {
    fn novo(tipo: u32) -> Arc<Socket> {
        Arc::new(Socket {
            tipo,
            estado: Mutex::new(Estado::Novo),
            endereco: Mutex::new(Endereco::Nenhum),
            chave: Mutex::new(None),
            caixa: Mutex::new(VecDeque::new()),
            credenciais: minhas_credenciais(),
        })
    }

    /// Onde dormem os que esperam por este socket (accept, recv de datagrama).
    fn canal(&self) -> usize {
        self as *const Socket as usize
    }

    /// Um par já conectado (o `socketpair`, e o `connect` + `accept`).
    fn conectar_par(a: &Socket, b: &Socket) {
        if a.tipo == SOCK_STREAM {
            let (ida, volta) = (Pipe::conectado(), Pipe::conectado());
            let par_de_a = b.endereco.lock().clone();
            let par_de_b = a.endereco.lock().clone();
            *a.estado.lock() = Estado::Conectado {
                entrada: volta.clone(),
                saida: ida.clone(),
                par: par_de_a,
                cred_par: b.credenciais,
                leitura: true,
                escrita: true,
            };
            *b.estado.lock() =
                Estado::Conectado { entrada: ida, saida: volta, par: par_de_b, cred_par: a.credenciais, leitura: true, escrita: true };
        }
    }

    pub fn prontidao(&self) -> u16 {
        match &*self.estado.lock() {
            Estado::Ouvindo { fila, .. } => {
                if fila.is_empty() { 0 } else { POLLIN }
            }
            Estado::Conectado { entrada, saida, .. } => {
                let mut r = entrada.prontidao(false);
                if r & POLLHUP != 0 {
                    r |= POLLIN; // o outro lado fechou: ler devolve 0 na hora
                }
                let w = saida.prontidao(true);
                if w & arquivo::POLLERR != 0 {
                    r |= POLLHUP | POLLOUT;
                } else {
                    r |= w;
                }
                r
            }
            Estado::Novo if self.tipo == SOCK_STREAM => POLLOUT | POLLHUP,
            _ => {
                let mut r = POLLOUT;
                if !self.caixa.lock().is_empty() {
                    r |= POLLIN;
                }
                r
            }
        }
    }

    /// `send`/`write`. `destino`: o endereço do `sendto` (só para datagramas).
    pub fn enviar(&self, dados: &[u8], destino: Option<Endereco>, bloquear: bool, sigpipe: bool) -> Result<usize, i64> {
        if self.tipo == SOCK_STREAM {
            let saida = match &*self.estado.lock() {
                Estado::Conectado { saida, escrita: true, .. } => saida.clone(),
                Estado::Conectado { .. } => {
                    if sigpipe {
                        let _ = processo::enviar_sinal(processo::atual().pid, processo::SIGPIPE);
                    }
                    return Err(EPIPE);
                }
                _ => return Err(ENOTCONN),
            };
            return saida.escrever(dados, bloquear, sigpipe);
        }
        // Datagrama: acha o socket de destino e põe a mensagem na caixa dele.
        let alvo = match destino {
            Some(endereco) => achar(&endereco)?,
            None => match &*self.estado.lock() {
                Estado::Destino { para, .. } => para.upgrade().ok_or(ECONNREFUSED)?,
                _ => return Err(EDESTADDRREQ),
            },
        };
        if alvo.tipo != SOCK_DGRAM {
            return Err(EPROTOTYPE);
        }
        if dados.len() > 64 * 1024 {
            return Err(EMSGSIZE);
        }
        let remetente = self.endereco.lock().clone();
        loop {
            {
                let mut caixa = alvo.caixa.lock();
                if caixa.len() < MAX_DATAGRAMAS {
                    caixa.push_back((dados.to_vec(), remetente));
                    break;
                }
            }
            if !bloquear {
                return Err(EAGAIN);
            }
            if processo::tem_sinal_pendente() {
                return Err(EINTR);
            }
            processo::dormir(alvo.canal());
        }
        processo::acordar(alvo.canal());
        Ok(dados.len())
    }

    /// `recv`/`read`. Devolve quantos bytes e (datagramas) de onde veio.
    pub fn receber(&self, buf: &mut [u8], bloquear: bool) -> Result<(usize, Endereco), i64> {
        if self.tipo == SOCK_STREAM {
            let (entrada, par) = match &*self.estado.lock() {
                Estado::Conectado { leitura: false, .. } => return Ok((0, Endereco::Nenhum)),
                Estado::Conectado { entrada, par, .. } => (entrada.clone(), par.clone()),
                _ => return Err(ENOTCONN),
            };
            return entrada.ler(buf, bloquear).map(|n| (n, par));
        }
        loop {
            if let Some((dados, de)) = self.caixa.lock().pop_front() {
                // O que não couber no buffer se perde (é assim com datagramas).
                let n = dados.len().min(buf.len());
                buf[..n].copy_from_slice(&dados[..n]);
                processo::acordar(self.canal()); // alguém pode estar esperando espaço
                return Ok((n, de));
            }
            if !bloquear {
                return Err(EAGAIN);
            }
            if processo::tem_sinal_pendente() {
                return Err(EINTR);
            }
            processo::dormir(self.canal());
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Estado::Conectado { entrada, saida, leitura, escrita, .. } = &*self.estado.lock() {
            if *leitura {
                entrada.soltar(false);
            }
            if *escrita {
                saida.soltar(true);
            }
        }
        // O endereço fica livre (o arquivo do socket continua lá, como no Unix:
        // quem for usar o caminho de novo apaga o arquivo antes).
        if let Some(chave) = self.chave.lock().take() {
            let mut ligados = LIGADOS.lock();
            if ligados.get(&chave).is_some_and(|w| w.strong_count() == 0) {
                ligados.remove(&chave);
            }
        }
    }
}

/// O socket ligado a um endereço (para o `connect` e o `sendto`).
fn achar(endereco: &Endereco) -> Result<Arc<Socket>, i64> {
    let chave = match endereco {
        Endereco::Nenhum => return Err(EINVAL),
        Endereco::Abstrato(nome) => Chave::Abstrato(nome.clone()),
        Endereco::Caminho(caminho) => {
            let no = vfs::buscar_como(caminho, cred())?;
            if no.tipo()? != vfs::Tipo::Socket {
                return Err(ECONNREFUSED);
            }
            no.permite(cred(), vfs::ESCREVER)?;
            Chave::No(no.id())
        }
    };
    LIGADOS.lock().get(&chave).and_then(Weak::upgrade).ok_or(ECONNREFUSED)
}

// -----------------------------------------------------------------------------
// Endereços: struct sockaddr_un { sa_family_t (2 bytes); char sun_path[108]; }
// -----------------------------------------------------------------------------

fn ler_endereco(ptr: u64, tamanho: u64) -> Result<Endereco, i64> {
    if !(2..=110).contains(&tamanho) {
        return Err(EINVAL);
    }
    let b = ler_usuario(ptr, tamanho as usize)?;
    if u16::from_le_bytes([b[0], b[1]]) as u64 != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let caminho = &b[2..];
    if caminho.is_empty() {
        return Ok(Endereco::Nenhum);
    }
    if caminho[0] == 0 {
        return Ok(Endereco::Abstrato(caminho[1..].to_vec()));
    }
    let fim = caminho.iter().position(|&c| c == 0).unwrap_or(caminho.len());
    let texto = core::str::from_utf8(&caminho[..fim]).map_err(|_| EINVAL)?;
    Ok(Endereco::Caminho(vfs::normalizar(&processo::atual().fs().diretorio, texto)))
}

/// Escreve o endereço em (ptr, *ptr_tamanho), como o accept/getsockname/recvfrom.
fn escrever_endereco(endereco: &Endereco, ptr: u64, ptr_tamanho: u64) -> Result<(), i64> {
    if ptr == 0 || ptr_tamanho == 0 {
        return Ok(());
    }
    let cabe = u32::from_le_bytes(ler_usuario(ptr_tamanho, 4)?.try_into().unwrap()) as usize;
    let mut b = Vec::from((AF_UNIX as u16).to_le_bytes());
    match endereco {
        Endereco::Nenhum => {}
        Endereco::Caminho(c) => {
            b.extend_from_slice(c.as_bytes());
            b.push(0);
        }
        Endereco::Abstrato(nome) => {
            b.push(0);
            b.extend_from_slice(nome);
        }
    }
    escrever_usuario(ptr, &b[..b.len().min(cabe)])?;
    escrever_usuario(ptr_tamanho, &(b.len() as u32).to_le_bytes())
}

// -----------------------------------------------------------------------------
// As chamadas de sistema
// -----------------------------------------------------------------------------

fn do_descritor(fd: u64) -> Result<(Arc<Arquivo>, Arc<Socket>), i64> {
    let arq = arquivo(fd)?;
    let Tipo::Socket(s) = &arq.tipo else { return Err(ENOTSOCK) };
    let s = s.clone();
    Ok((arq, s))
}

fn descritor_para(s: Arc<Socket>, flags: u64) -> Resultado {
    descritor_com_flags(Tipo::Socket(s), flags)
}

/// Um descritor novo para um socket, com SOCK_NONBLOCK e SOCK_CLOEXEC.
pub(crate) fn descritor_com_flags(tipo: Tipo, flags: u64) -> Resultado {
    let arq = Arquivo::novo(tipo, arquivo::O_RDWR | flags & SOCK_NONBLOCK);
    let fd = novo_descritor(arq)?;
    if flags & SOCK_CLOEXEC != 0 {
        processo::atual().arquivos().fecha_no_exec |= 1 << fd;
    }
    Ok(fd)
}

fn tipo_valido(dominio: u64, tipo: u64) -> Result<u32, i64> {
    if dominio != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    match (tipo & 0xF) as u32 {
        t @ (SOCK_STREAM | SOCK_DGRAM) => Ok(t),
        _ => Err(ESOCKTNOSUPPORT),
    }
}

pub fn socket(dominio: u64, tipo: u64) -> Resultado {
    let t = tipo_valido(dominio, tipo)?;
    descritor_para(Socket::novo(t), tipo)
}

pub fn socketpair(dominio: u64, tipo: u64, ptr: u64) -> Resultado {
    let t = tipo_valido(dominio, tipo)?;
    let (a, b) = (Socket::novo(t), Socket::novo(t));
    if t == SOCK_STREAM {
        Socket::conectar_par(&a, &b);
    } else {
        *a.estado.lock() = Estado::Destino { para: Arc::downgrade(&b), endereco: Endereco::Nenhum };
        *b.estado.lock() = Estado::Destino { para: Arc::downgrade(&a), endereco: Endereco::Nenhum };
    }
    let fd_a = descritor_para(a, tipo)?;
    let fd_b = match descritor_para(b, tipo) {
        Ok(fd) => fd,
        Err(e) => {
            processo::atual().arquivos().lista[fd_a as usize] = None;
            return Err(e);
        }
    };
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&(fd_a as i32).to_le_bytes());
    bytes[4..].copy_from_slice(&(fd_b as i32).to_le_bytes());
    escrever_usuario(ptr, &bytes)?;
    Ok(0)
}

pub fn bind(fd: u64, ptr: u64, tamanho: u64) -> Resultado {
    let (_, s) = do_descritor(fd)?;
    if *s.endereco.lock() != Endereco::Nenhum {
        return Err(EINVAL); // já ligado
    }
    let endereco = ler_endereco(ptr, tamanho)?;
    let chave = match &endereco {
        Endereco::Nenhum => return Err(EINVAL),
        Endereco::Abstrato(nome) => {
            let chave = Chave::Abstrato(nome.clone());
            if LIGADOS.lock().get(&chave).is_some_and(|w| w.strong_count() > 0) {
                return Err(EADDRINUSE);
            }
            chave
        }
        Endereco::Caminho(caminho) => {
            pai_gravavel(caminho)?;
            let p = processo::atual();
            let permissoes = 0o777 & !p.fs().umask;
            let meta = Meta { permissoes, uid: p.euid, gid: p.egid, modificado: crate::rtc::agora() };
            let no = vfs::criar_socket(caminho, meta).map_err(|e| if e == EEXIST { EADDRINUSE } else { e })?;
            Chave::No(no.id())
        }
    };
    LIGADOS.lock().insert(chave.clone(), Arc::downgrade(&s));
    *s.chave.lock() = Some(chave);
    *s.endereco.lock() = endereco;
    Ok(0)
}

pub fn listen(fd: u64, maximo: u64) -> Resultado {
    let (_, s) = do_descritor(fd)?;
    if s.tipo != SOCK_STREAM {
        return Err(EOPNOTSUPP);
    }
    if *s.endereco.lock() == Endereco::Nenhum {
        return Err(EINVAL);
    }
    let maximo = (maximo as usize).clamp(1, 128);
    let mut estado = s.estado.lock();
    match &mut *estado {
        Estado::Novo => *estado = Estado::Ouvindo { fila: VecDeque::new(), maximo },
        Estado::Ouvindo { maximo: m, .. } => *m = maximo,
        _ => return Err(EINVAL),
    }
    Ok(0)
}

pub fn connect(fd: u64, ptr: u64, tamanho: u64) -> Resultado {
    let (arq, s) = do_descritor(fd)?;
    let endereco = ler_endereco(ptr, tamanho)?;
    let alvo = achar(&endereco)?;
    if alvo.tipo != s.tipo {
        return Err(EPROTOTYPE);
    }
    if s.tipo == SOCK_DGRAM {
        *s.estado.lock() = Estado::Destino { para: Arc::downgrade(&alvo), endereco };
        return Ok(0);
    }
    if !matches!(*s.estado.lock(), Estado::Novo) {
        return Err(EISCONN);
    }
    // O lado do servidor nasce já conectado e entra na fila do accept.
    let servidor = Socket::novo(SOCK_STREAM);
    *servidor.endereco.lock() = endereco;
    loop {
        {
            let mut estado = alvo.estado.lock();
            let Estado::Ouvindo { fila, maximo } = &mut *estado else { return Err(ECONNREFUSED) };
            if fila.len() < *maximo {
                Socket::conectar_par(&s, &servidor);
                // as credenciais do servidor são as de quem ouve, não as nossas
                if let Estado::Conectado { cred_par, .. } = &mut *s.estado.lock() {
                    *cred_par = alvo.credenciais;
                }
                fila.push_back(servidor);
                break;
            }
        }
        if !arq.bloqueia() {
            return Err(EAGAIN);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(alvo.canal());
    }
    processo::acordar(alvo.canal());
    Ok(0)
}

pub fn accept(fd: u64, ptr: u64, ptr_tamanho: u64, flags: u64) -> Resultado {
    let (arq, s) = do_descritor(fd)?;
    loop {
        {
            let mut estado = s.estado.lock();
            let Estado::Ouvindo { fila, .. } = &mut *estado else { return Err(EINVAL) };
            if let Some(novo) = fila.pop_front() {
                drop(estado);
                processo::acordar(s.canal()); // abriu vaga na fila
                // O endereço do outro lado é o do cliente (em geral, nenhum).
                let cliente = match &*novo.estado.lock() {
                    Estado::Conectado { par, .. } => par.clone(),
                    _ => Endereco::Nenhum,
                };
                let fd = descritor_para(novo, flags)?;
                escrever_endereco(&cliente, ptr, ptr_tamanho)?;
                return Ok(fd);
            }
        }
        if !arq.bloqueia() {
            return Err(EAGAIN);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(s.canal());
    }
}

pub fn sendto(fd: u64, buf: u64, tamanho: u64, flags: u64, ptr: u64, ptr_tamanho: u64) -> Resultado {
    let (arq, s) = do_descritor(fd)?;
    let dados = ler_usuario(buf, tamanho.min(1 << 20) as usize)?;
    let destino = if ptr != 0 { Some(ler_endereco(ptr, ptr_tamanho)?) } else { None };
    let bloquear = arq.bloqueia() && flags & MSG_DONTWAIT == 0;
    s.enviar(&dados, destino, bloquear, flags & MSG_NOSIGNAL == 0).map(|n| n as u64)
}

pub fn recvfrom(fd: u64, buf: u64, tamanho: u64, flags: u64, ptr: u64, ptr_tamanho: u64) -> Resultado {
    let (arq, s) = do_descritor(fd)?;
    let mut dados = alloc::vec![0u8; tamanho.min(1 << 20) as usize];
    let bloquear = arq.bloqueia() && flags & MSG_DONTWAIT == 0;
    let (n, de) = s.receber(&mut dados, bloquear)?;
    escrever_usuario(buf, &dados[..n])?;
    escrever_endereco(&de, ptr, ptr_tamanho)?;
    Ok(n as u64)
}

/// struct msghdr { name, namelen, iov, iovlen, control, controllen, flags }
pub(crate) fn ler_msghdr(ptr: u64) -> Result<(u64, u64, Vec<(u64, u64)>), i64> {
    let m = ler_usuario(ptr, 56)?;
    let campo = |i: usize| u64::from_le_bytes(m[i..i + 8].try_into().unwrap());
    let (nome, tamanho_nome, iov, quantos) = (campo(0), campo(8) & 0xFFFF_FFFF, campo(16), campo(24));
    if quantos > 1024 {
        return Err(EINVAL);
    }
    let bruto = ler_usuario(iov, quantos as usize * 16)?;
    let vetores = bruto
        .chunks_exact(16)
        .map(|c| (u64::from_le_bytes(c[..8].try_into().unwrap()), u64::from_le_bytes(c[8..].try_into().unwrap())))
        .collect();
    Ok((nome, tamanho_nome, vetores))
}

pub fn sendmsg(fd: u64, ptr: u64, flags: u64) -> Resultado {
    let (arq, s) = do_descritor(fd)?;
    let (nome, tamanho_nome, vetores) = ler_msghdr(ptr)?;
    let mut dados = Vec::new();
    for (base, n) in vetores {
        dados.extend(ler_usuario(base, n.min(1 << 20) as usize)?);
    }
    let destino = if nome != 0 { Some(ler_endereco(nome, tamanho_nome)?) } else { None };
    let bloquear = arq.bloqueia() && flags & MSG_DONTWAIT == 0;
    s.enviar(&dados, destino, bloquear, flags & MSG_NOSIGNAL == 0).map(|n| n as u64)
}

pub fn recvmsg(fd: u64, ptr: u64, flags: u64) -> Resultado {
    let (arq, s) = do_descritor(fd)?;
    let (nome, _, vetores) = ler_msghdr(ptr)?;
    let total: u64 = vetores.iter().map(|v| v.1).sum();
    let mut dados = alloc::vec![0u8; total.min(1 << 20) as usize];
    let bloquear = arq.bloqueia() && flags & MSG_DONTWAIT == 0;
    let (n, de) = s.receber(&mut dados, bloquear)?;
    let mut feito = 0;
    for (base, tamanho) in vetores {
        let parte = (tamanho as usize).min(n - feito);
        escrever_usuario(base, &dados[feito..feito + parte])?;
        feito += parte;
    }
    escrever_endereco(&de, nome, ptr + 8)?;
    escrever_usuario(ptr + 40, &0u64.to_le_bytes())?; // msg_controllen: sem mensagens de controle
    escrever_usuario(ptr + 48, &0u32.to_le_bytes())?; // msg_flags
    Ok(n as u64)
}

pub fn shutdown(fd: u64, como: u64) -> Resultado {
    let (_, s) = do_descritor(fd)?;
    let mut estado = s.estado.lock();
    let Estado::Conectado { entrada, saida, leitura, escrita, .. } = &mut *estado else { return Err(ENOTCONN) };
    // 0 = não vou mais ler, 1 = não vou mais escrever (o outro lado lê fim de arquivo), 2 = os dois
    if como != 1 && core::mem::take(leitura) {
        entrada.soltar(false);
    }
    if como != 0 && core::mem::take(escrita) {
        saida.soltar(true);
    }
    Ok(0)
}

pub fn getsockname(fd: u64, ptr: u64, ptr_tamanho: u64) -> Resultado {
    let (_, s) = do_descritor(fd)?;
    let endereco = s.endereco.lock().clone();
    escrever_endereco(&endereco, ptr, ptr_tamanho)?;
    Ok(0)
}

pub fn getpeername(fd: u64, ptr: u64, ptr_tamanho: u64) -> Resultado {
    let (_, s) = do_descritor(fd)?;
    let par = match &*s.estado.lock() {
        Estado::Conectado { par, .. } => par.clone(),
        Estado::Destino { endereco, .. } => endereco.clone(),
        _ => return Err(ENOTCONN),
    };
    escrever_endereco(&par, ptr, ptr_tamanho)?;
    Ok(0)
}

pub fn getsockopt(fd: u64, nivel: u64, opcao: u64, ptr: u64, ptr_tamanho: u64) -> Resultado {
    const SOL_SOCKET: u64 = 1;
    const SO_TYPE: u64 = 3;
    const SO_ERROR: u64 = 4;
    const SO_SNDBUF: u64 = 7;
    const SO_RCVBUF: u64 = 8;
    const SO_PEERCRED: u64 = 17;
    let (_, s) = do_descritor(fd)?;
    if nivel != SOL_SOCKET {
        return Err(ENOPROTOOPT);
    }
    let valor: Vec<u8> = match opcao {
        SO_TYPE => s.tipo.to_le_bytes().to_vec(),
        SO_ERROR => 0u32.to_le_bytes().to_vec(),
        SO_SNDBUF | SO_RCVBUF => 4096u32.to_le_bytes().to_vec(),
        SO_PEERCRED => {
            let (pid, uid, gid) = match &*s.estado.lock() {
                Estado::Conectado { cred_par, .. } => *cred_par,
                _ => return Err(ENOTCONN),
            };
            [pid, uid, gid].iter().flat_map(|v| v.to_le_bytes()).collect()
        }
        _ => return Err(ENOPROTOOPT),
    };
    let cabe = u32::from_le_bytes(ler_usuario(ptr_tamanho, 4)?.try_into().unwrap()) as usize;
    escrever_usuario(ptr, &valor[..valor.len().min(cabe)])?;
    escrever_usuario(ptr_tamanho, &(valor.len().min(cabe) as u32).to_le_bytes())?;
    Ok(0)
}
