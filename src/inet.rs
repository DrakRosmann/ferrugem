//! Sockets de rede (`AF_INET`): TCP (`SOCK_STREAM`), UDP (`SOCK_DGRAM`) e o
//! ping (`SOCK_DGRAM` com `IPPROTO_ICMP`, como o "ping sem root" do Linux:
//! o programa manda e recebe mensagens ICMP de eco; o kernel cuida do id).
//!
//! Os endereços são `struct sockaddr_in`:
//!
//! ```text
//!   u16 família = 2 (AF_INET) | u16 porta (big-endian) | u8[4] IP | 8 zeros
//! ```
//!
//! IP 0.0.0.0 no `bind` quer dizer "qualquer um dos nossos"; no `connect`,
//! a própria máquina.

use crate::arquivo::{Arquivo, Tipo, POLLERR, POLLHUP, POLLIN, POLLOUT};
use crate::erro::*;
use crate::processo;
use crate::rede::{self, Ip};
use crate::socket::{descritor_com_flags, ler_msghdr, SOCK_DGRAM, SOCK_STREAM};
use crate::spin::Mutex;
use crate::syscall::{arquivo, escrever_usuario, ler_usuario, Resultado};
use crate::tcp::{self, Ouvinte, Ponta, Tcb};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

pub const AF_INET: u64 = 2;
const MSG_DONTWAIT: u64 = 0x40;
const MSG_NOSIGNAL: u64 = 0x4000;
const MSG_PEEK: u64 = 0x2;
const MAX_DATAGRAMAS: usize = 128;

enum Lado {
    Nada,
    Conexao(Arc<Mutex<Tcb>>),
    Ouvinte(Arc<Ouvinte>),
}

pub struct SocketInet {
    pub tipo: u32,
    protocolo: u8,
    /// A porta 0 = ainda sem porta.
    local: Mutex<Ponta>,
    /// O destino padrão de um datagrama (`connect` num SOCK_DGRAM).
    remoto: Mutex<Option<Ponta>>,
    caixa: Mutex<VecDeque<(Vec<u8>, Ponta)>>,
    lado: Mutex<Lado>,
}

/// Os sockets de datagramas com porta, para entregar o que chega.
static DATAGRAMAS: Mutex<Vec<Weak<SocketInet>>> = Mutex::new(Vec::new());
static PROXIMA_PORTA: AtomicU16 = AtomicU16::new(0);

fn porta_em_uso(protocolo: u8, porta: u16) -> bool {
    if protocolo == rede::TCP {
        return tcp::porta_em_uso(porta);
    }
    DATAGRAMAS.lock().iter().filter_map(Weak::upgrade).any(|s| s.protocolo == protocolo && s.local.lock().1 == porta)
}

/// Uma porta livre na faixa "efêmera" (49152 a 65535).
fn porta_efemera(protocolo: u8) -> Result<u16, i64> {
    if PROXIMA_PORTA.load(Ordering::Relaxed) == 0 {
        PROXIMA_PORTA.store(49152 + (crate::aleatorio::proximo() % 16384) as u16, Ordering::Relaxed);
    }
    for _ in 0..16384 {
        let p = PROXIMA_PORTA.fetch_add(1, Ordering::Relaxed);
        let p = if p < 49152 { PROXIMA_PORTA.store(49153, Ordering::Relaxed); 49152 } else { p };
        if !porta_em_uso(protocolo, p) {
            return Ok(p);
        }
    }
    Err(EADDRINUSE)
}

/// Os sockets de datagramas com porta: (protocolo, local).
pub fn datagramas() -> Vec<(u8, Ponta)> {
    DATAGRAMAS.lock().iter().filter_map(Weak::upgrade).map(|s| (s.protocolo, *s.local.lock())).collect()
}

/// Um datagrama (UDP, ou resposta de eco) chegou para a `porta`.
pub fn entregar_datagrama(protocolo: u8, porta: u16, dados: &[u8], de: Ponta) {
    let alvo = DATAGRAMAS.lock().iter().filter_map(Weak::upgrade).find(|s| {
        s.protocolo == protocolo
            && s.local.lock().1 == porta
            // conectado: só aceita de quem escolheu
            && s.remoto.lock().is_none_or(|r| protocolo == rede::ICMP || r == de)
    });
    if let Some(s) = alvo {
        let mut caixa = s.caixa.lock();
        if caixa.len() < MAX_DATAGRAMAS {
            caixa.push_back((dados.to_vec(), de));
        }
        drop(caixa);
        processo::acordar(s.canal());
    }
}

impl SocketInet {
    fn novo(tipo: u32, protocolo: u8) -> Arc<SocketInet> {
        Arc::new(SocketInet {
            tipo,
            protocolo,
            local: Mutex::new((0, 0)),
            remoto: Mutex::new(None),
            caixa: Mutex::new(VecDeque::new()),
            lado: Mutex::new(Lado::Nada),
        })
    }

    fn canal(&self) -> usize {
        self as *const SocketInet as usize
    }

    fn conexao(&self) -> Option<Arc<Mutex<Tcb>>> {
        match &*self.lado.lock() {
            Lado::Conexao(c) => Some(c.clone()),
            _ => None,
        }
    }

    /// Dá uma porta ao socket (a pedida, ou uma efêmera se for 0).
    fn ligar(self: &Arc<Self>, ip: Ip, porta: u16) -> Result<(), i64> {
        if self.local.lock().1 != 0 {
            return Err(EINVAL);
        }
        let porta = if porta == 0 {
            porta_efemera(self.protocolo)?
        } else if porta_em_uso(self.protocolo, porta) {
            return Err(EADDRINUSE);
        } else {
            porta
        };
        *self.local.lock() = (ip, porta);
        if self.tipo == SOCK_DGRAM {
            let mut d = DATAGRAMAS.lock();
            d.retain(|w| w.strong_count() > 0);
            d.push(Arc::downgrade(self));
        }
        Ok(())
    }

    pub fn prontidao(&self) -> u16 {
        match &*self.lado.lock() {
            Lado::Ouvinte(o) => {
                if o.fila.lock().is_empty() { 0 } else { POLLIN }
            }
            Lado::Conexao(c) => {
                let t = c.lock();
                let mut r = 0;
                if !t.entrada.is_empty() || t.fin_recebido || t.erro != 0 || t.estado == tcp::Estado::Fechado {
                    r |= POLLIN;
                }
                if t.escrita_aberta() && t.cabe_escrever() {
                    r |= POLLOUT;
                }
                if t.erro != 0 {
                    r |= POLLERR | POLLOUT;
                }
                if t.estado == tcp::Estado::Fechado || t.fin_recebido && !t.escrita_aberta() {
                    r |= POLLHUP;
                }
                r
            }
            Lado::Nada if self.tipo == SOCK_STREAM => POLLOUT | POLLHUP,
            Lado::Nada => {
                if self.caixa.lock().is_empty() { POLLOUT } else { POLLIN | POLLOUT }
            }
        }
    }

    /// Dorme até um evento no canal. Quem chama confere o estado depois de
    /// `rede::processar_loopback()` e antes de vir aqui: processar o loopback
    /// entre a conferência e o sono perderia o aviso (ninguém dormindo ainda).
    fn esperar(&self, canal: usize) -> Result<(), i64> {
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(canal);
        Ok(())
    }

    pub fn enviar(self: &Arc<Self>, dados: &[u8], destino: Option<Ponta>, bloquear: bool, sigpipe: bool) -> Result<usize, i64> {
        let r = self.enviar_sem_loopback(dados, destino, bloquear, sigpipe);
        rede::processar_loopback();
        r
    }

    fn enviar_sem_loopback(self: &Arc<Self>, dados: &[u8], destino: Option<Ponta>, bloquear: bool, sigpipe: bool) -> Result<usize, i64> {
        if self.tipo == SOCK_STREAM {
            let c = self.conexao().ok_or(ENOTCONN)?;
            let mut feito = 0;
            loop {
                rede::processar_loopback();
                {
                    let mut t = c.lock();
                    if t.erro != 0 {
                        return Err(t.erro);
                    }
                    if !t.conectando() {
                        if !t.escrita_aberta() {
                            if sigpipe {
                                let _ = processo::enviar_sinal(processo::atual().pid, processo::SIGPIPE);
                            }
                            return Err(EPIPE);
                        }
                        feito += t.escrever(&dados[feito..]);
                        if feito == dados.len() {
                            return Ok(feito);
                        }
                    }
                }
                if !bloquear {
                    return if feito > 0 { Ok(feito) } else { Err(EAGAIN) };
                }
                if let Err(e) = self.esperar(tcp::canal(&c)) {
                    return if feito > 0 { Ok(feito) } else { Err(e) };
                }
            }
        }
        // Datagrama
        let destino = match destino.or(*self.remoto.lock()) {
            Some((0, porta)) => (rede::LOOPBACK, porta),
            Some(d) => d,
            None => return Err(EDESTADDRREQ),
        };
        if !rede::e_local(destino.0) && !rede::tem_placa() {
            return Err(ENETUNREACH);
        }
        if self.local.lock().1 == 0 {
            self.ligar(0, 0)?;
        }
        let local = *self.local.lock();
        let origem = if local.0 == 0 { rede::origem_para(destino.0) } else { local.0 };
        match self.protocolo {
            rede::ICMP => rede::enviar_eco(origem, destino.0, local.1, dados)?,
            _ => rede::enviar_udp((origem, local.1), destino, dados)?,
        }
        Ok(dados.len())
    }

    pub fn receber(&self, buf: &mut [u8], bloquear: bool, espiar: bool) -> Result<(usize, Ponta), i64> {
        if self.tipo == SOCK_STREAM {
            let c = self.conexao().ok_or(ENOTCONN)?;
            loop {
                rede::processar_loopback();
                {
                    let mut t = c.lock();
                    if !t.entrada.is_empty() {
                        let n = if espiar {
                            let n = buf.len().min(t.entrada.len());
                            for (d, b) in buf.iter_mut().zip(t.entrada.iter()) {
                                *d = *b;
                            }
                            n
                        } else {
                            t.ler(buf)
                        };
                        return Ok((n, t.remoto));
                    }
                    if t.fin_recebido || t.leitura_fechada {
                        return Ok((0, t.remoto));
                    }
                    if t.erro != 0 {
                        return Err(t.erro);
                    }
                    if t.estado == tcp::Estado::Fechado {
                        return Ok((0, t.remoto));
                    }
                }
                if !bloquear {
                    return Err(EAGAIN);
                }
                self.esperar(tcp::canal(&c))?;
            }
        }
        loop {
            rede::processar_loopback();
            {
                let mut caixa = self.caixa.lock();
                if let Some((dados, de)) = caixa.front() {
                    let n = dados.len().min(buf.len());
                    buf[..n].copy_from_slice(&dados[..n]);
                    let de = *de;
                    if !espiar {
                        caixa.pop_front();
                    }
                    return Ok((n, de));
                }
            }
            if !bloquear {
                return Err(EAGAIN);
            }
            self.esperar(self.canal())?;
        }
    }
}

impl Drop for SocketInet {
    fn drop(&mut self) {
        if let Lado::Conexao(c) = &*self.lado.lock() {
            tcp::abandonar(c);
        }
        // O ouvinte (se houver) cai junto com a última referência.
    }
}

// -----------------------------------------------------------------------------
// Endereços
// -----------------------------------------------------------------------------

fn ler_endereco(ptr: u64, tamanho: u64) -> Result<Ponta, i64> {
    if tamanho < 8 {
        return Err(EINVAL);
    }
    let b = ler_usuario(ptr, 8)?;
    if u16::from_le_bytes([b[0], b[1]]) as u64 != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    Ok((u32::from_be_bytes(b[4..8].try_into().unwrap()), u16::from_be_bytes([b[2], b[3]])))
}

fn escrever_endereco(p: Ponta, ptr: u64, ptr_tamanho: u64) -> Result<(), i64> {
    if ptr == 0 || ptr_tamanho == 0 {
        return Ok(());
    }
    let cabe = u32::from_le_bytes(ler_usuario(ptr_tamanho, 4)?.try_into().unwrap()) as usize;
    let mut b = [0u8; 16];
    b[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    b[2..4].copy_from_slice(&p.1.to_be_bytes());
    b[4..8].copy_from_slice(&p.0.to_be_bytes());
    escrever_usuario(ptr, &b[..cabe.min(16)])?;
    escrever_usuario(ptr_tamanho, &16u32.to_le_bytes())
}

// -----------------------------------------------------------------------------
// As chamadas de sistema
// -----------------------------------------------------------------------------

/// O descritor é um socket de rede?
pub fn e_inet(fd: u64) -> bool {
    arquivo(fd).is_ok_and(|a| matches!(a.tipo, Tipo::Inet(_)))
}

fn do_descritor(fd: u64) -> Result<(Arc<Arquivo>, Arc<SocketInet>), i64> {
    let arq = arquivo(fd)?;
    let Tipo::Inet(s) = &arq.tipo else { return Err(ENOTSOCK) };
    let s = s.clone();
    Ok((arq, s))
}

pub fn socket(tipo: u64, protocolo: u64) -> Resultado {
    let t = (tipo & 0xF) as u32;
    let p = match (t, protocolo) {
        (SOCK_STREAM, 0 | 6) => rede::TCP,
        (SOCK_DGRAM, 0 | 17) => rede::UDP,
        (SOCK_DGRAM, 1) => rede::ICMP,
        (SOCK_STREAM | SOCK_DGRAM, _) => return Err(EPROTONOSUPPORT),
        _ => return Err(ESOCKTNOSUPPORT),
    };
    descritor_com_flags(Tipo::Inet(SocketInet::novo(t, p)), tipo)
}

/// As chamadas que recebem um descritor (o syscall.rs só chama esta se o
/// descritor for um socket de rede).
pub fn chamada(numero: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> Resultado {
    let (arq, s) = do_descritor(a1)?;
    let r = match numero {
        42 => connect(&arq, &s, a2, a3),
        43 => accept(&arq, &s, a2, a3, 0),
        288 => accept(&arq, &s, a2, a3, a4),
        44 => {
            let dados = ler_usuario(a2, a3.min(1 << 20) as usize)?;
            let destino = if a5 != 0 { Some(ler_endereco(a5, a6)?) } else { None };
            let bloquear = arq.bloqueia() && a4 & MSG_DONTWAIT == 0;
            s.enviar(&dados, destino, bloquear, a4 & MSG_NOSIGNAL == 0).map(|n| n as u64)
        }
        45 => {
            let mut dados = alloc::vec![0u8; a3.min(1 << 20) as usize];
            let bloquear = arq.bloqueia() && a4 & MSG_DONTWAIT == 0;
            let (n, de) = s.receber(&mut dados, bloquear, a4 & MSG_PEEK != 0)?;
            escrever_usuario(a2, &dados[..n])?;
            escrever_endereco(de, a5, a6)?;
            Ok(n as u64)
        }
        46 => {
            let (nome, tamanho_nome, vetores) = ler_msghdr(a2)?;
            let mut dados = Vec::new();
            for (base, n) in vetores {
                dados.extend(ler_usuario(base, n.min(1 << 20) as usize)?);
            }
            let destino = if nome != 0 { Some(ler_endereco(nome, tamanho_nome)?) } else { None };
            let bloquear = arq.bloqueia() && a3 & MSG_DONTWAIT == 0;
            s.enviar(&dados, destino, bloquear, a3 & MSG_NOSIGNAL == 0).map(|n| n as u64)
        }
        47 => {
            let (nome, _, vetores) = ler_msghdr(a2)?;
            let total: u64 = vetores.iter().map(|v| v.1).sum();
            let mut dados = alloc::vec![0u8; total.min(1 << 20) as usize];
            let bloquear = arq.bloqueia() && a3 & MSG_DONTWAIT == 0;
            let (n, de) = s.receber(&mut dados, bloquear, a3 & MSG_PEEK != 0)?;
            let mut feito = 0;
            for (base, tamanho) in vetores {
                let parte = (tamanho as usize).min(n - feito);
                escrever_usuario(base, &dados[feito..feito + parte])?;
                feito += parte;
            }
            escrever_endereco(de, nome, a2 + 8)?;
            escrever_usuario(a2 + 40, &0u64.to_le_bytes())?;
            escrever_usuario(a2 + 48, &0u32.to_le_bytes())?;
            Ok(n as u64)
        }
        48 => shutdown(&s, a2),
        49 => bind(&s, a2, a3),
        50 => listen(&s, a2),
        51 => {
            let local = *s.local.lock();
            escrever_endereco(local, a2, a3).map(|_| 0)
        }
        52 => {
            let par = match s.conexao() {
                Some(c) => c.lock().remoto,
                None => s.remoto.lock().ok_or(ENOTCONN)?,
            };
            escrever_endereco(par, a2, a3).map(|_| 0)
        }
        54 => Ok(0), // setsockopt: aceitas e ignoradas (SO_REUSEADDR, TCP_NODELAY...)
        55 => getsockopt(&s, a2, a3, a4, a5),
        _ => Err(ENOSYS),
    };
    rede::processar_loopback();
    r
}

fn bind(s: &Arc<SocketInet>, ptr: u64, tamanho: u64) -> Resultado {
    let (ip, porta) = ler_endereco(ptr, tamanho)?;
    if ip != 0 && !rede::e_local(ip) {
        return Err(EADDRNOTAVAIL);
    }
    s.ligar(ip, porta)?;
    Ok(0)
}

fn listen(s: &Arc<SocketInet>, maximo: u64) -> Resultado {
    if s.tipo != SOCK_STREAM {
        return Err(EOPNOTSUPP);
    }
    let mut lado = s.lado.lock();
    match &*lado {
        Lado::Nada => {}
        Lado::Ouvinte(_) => return Ok(0),
        Lado::Conexao(_) => return Err(EINVAL),
    }
    if s.local.lock().1 == 0 {
        *s.local.lock() = (0, porta_efemera(rede::TCP)?);
    }
    *lado = Lado::Ouvinte(tcp::ouvir(*s.local.lock(), maximo as usize)?);
    Ok(0)
}

fn connect(arq: &Arquivo, s: &Arc<SocketInet>, ptr: u64, tamanho: u64) -> Resultado {
    let (mut ip, porta) = ler_endereco(ptr, tamanho)?;
    if ip == 0 {
        ip = rede::LOOPBACK;
    }
    if !rede::e_local(ip) && !rede::tem_placa() {
        return Err(ENETUNREACH);
    }
    if s.tipo == SOCK_DGRAM {
        if s.local.lock().1 == 0 {
            s.ligar(0, 0)?;
        }
        *s.remoto.lock() = Some((ip, porta));
        return Ok(0);
    }
    let c = {
        let mut lado = s.lado.lock();
        match &*lado {
            Lado::Conexao(c) => return Err(if c.lock().conectando() { EALREADY } else { EISCONN }),
            Lado::Ouvinte(_) => return Err(EINVAL),
            Lado::Nada => {}
        }
        let mut local = *s.local.lock();
        if local.1 == 0 {
            local.1 = porta_efemera(rede::TCP)?;
        }
        if local.0 == 0 {
            local.0 = rede::origem_para(ip);
        }
        *s.local.lock() = local;
        let c = tcp::conectar(local, (ip, porta));
        *lado = Lado::Conexao(c.clone());
        c
    };
    if !arq.bloqueia() {
        return Err(EINPROGRESS);
    }
    loop {
        rede::processar_loopback();
        {
            let t = c.lock();
            if t.estado == tcp::Estado::Fechado {
                return Err(if t.erro != 0 { t.erro } else { ECONNREFUSED });
            }
            if !t.conectando() {
                return Ok(0);
            }
        }
        s.esperar(tcp::canal(&c))?;
    }
}

fn accept(arq: &Arquivo, s: &Arc<SocketInet>, ptr: u64, ptr_tamanho: u64, flags: u64) -> Resultado {
    let o = match &*s.lado.lock() {
        Lado::Ouvinte(o) => o.clone(),
        _ => return Err(EINVAL),
    };
    loop {
        rede::processar_loopback();
        let primeira = o.fila.lock().pop_front();
        if let Some(c) = primeira {
            let (local, remoto) = {
                let t = c.lock();
                (t.local, t.remoto)
            };
            let novo = SocketInet::novo(SOCK_STREAM, rede::TCP);
            *novo.local.lock() = local;
            *novo.lado.lock() = Lado::Conexao(c);
            let fd = descritor_com_flags(Tipo::Inet(novo), flags)?;
            escrever_endereco(remoto, ptr, ptr_tamanho)?;
            return Ok(fd);
        }
        if !arq.bloqueia() {
            return Err(EAGAIN);
        }
        s.esperar(tcp::canal_ouvinte(&o))?;
    }
}

fn shutdown(s: &SocketInet, como: u64) -> Resultado {
    let c = s.conexao().ok_or(ENOTCONN)?;
    let mut t = c.lock();
    // 0 = não vou mais ler, 1 = não vou mais escrever, 2 = os dois
    if como != 1 {
        t.leitura_fechada = true;
        t.entrada.clear();
    }
    if como != 0 {
        t.fechar_escrita();
    }
    drop(t);
    processo::acordar(tcp::canal(&c));
    Ok(0)
}

fn getsockopt(s: &SocketInet, nivel: u64, opcao: u64, ptr: u64, ptr_tamanho: u64) -> Resultado {
    const SOL_SOCKET: u64 = 1;
    let valor: u32 = match (nivel, opcao) {
        (SOL_SOCKET, 3) => s.tipo,                                            // SO_TYPE
        (SOL_SOCKET, 4) => s.conexao().map_or(0, |c| c.lock().erro) as u32,   // SO_ERROR
        (SOL_SOCKET, 7 | 8) => tcp::CAPACIDADE_SAIDA as u32,                  // SO_SNDBUF, SO_RCVBUF
        (SOL_SOCKET, _) | (6, _) => 0,                                        // o resto (e o nível TCP): desligado
        _ => return Err(ENOPROTOOPT),
    };
    let cabe = u32::from_le_bytes(ler_usuario(ptr_tamanho, 4)?.try_into().unwrap()) as usize;
    escrever_usuario(ptr, &valor.to_le_bytes()[..cabe.min(4)])?;
    escrever_usuario(ptr_tamanho, &(cabe.min(4) as u32).to_le_bytes())?;
    Ok(0)
}
