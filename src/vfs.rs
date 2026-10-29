//! Sistema de arquivos virtual (VFS): uma árvore única de diretórios, não
//! importa onde os dados estejam.
//!
//! - A raiz (`/`) vive na memória (como o tmpfs do Linux): na partida,
//!   o conteúdo do initrd é copiado para cá. Tudo some ao desligar.
//! - Em `/disco` fica *montado* o disco ext2: o que se grava ali sobrevive.
//!
//! Quem usa o VFS não precisa saber a diferença: tudo é um `Vnode`, com as
//! mesmas operações (ler, escrever, listar, criar...). É a mesma ideia do
//! VFS do Linux, que esconde dezenas de sistemas de arquivos atrás de uma
//! interface só.
//!
//! Os caminhos que chegam aqui já estão normalizados e absolutos
//! (`/home/../bin/./ls` vira `/bin/ls`; veja `normalizar`).
//!
//! ## Links
//!
//! Um link *simbólico* é um arquivo que contém um caminho; ao encontrá-lo no
//! meio de uma busca, continuamos a partir do caminho que ele guarda. Um link
//! *físico* é só mais um nome para o mesmo nó (o mesmo `Arc<No>`, ou o mesmo
//! inode no ext2): o arquivo só some quando o último nome for apagado.
//!
//! ## Permissões
//!
//! Todo arquivo tem um dono, um grupo e 9 bits de permissão, em três trios:
//! dono, grupo e "os outros". `rwxr-x---` = o dono lê, escreve e executa; o
//! grupo lê e executa; os outros, nada. Num diretório, `r` deixa listar, `w`
//! deixa criar e apagar dentro dele, e `x` deixa *atravessá-lo* num caminho.
//! Mais três bits especiais: setuid (o programa roda com a identidade do dono
//! do arquivo), setgid, e sticky (num diretório como o /tmp: cada um só apaga
//! o que é seu). O root (uid 0) pode tudo, menos executar um arquivo sem nenhum `x`.

use crate::erro::*;
use crate::ext2::{self, Ext2};
use crate::procfs::NoProc;
use crate::rtc;
use crate::spin::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dispositivo {
    Terminal,
    Nulo,
    Zero,
    /// /dev/random e /dev/urandom
    Aleatorio,
    /// Um disco ou partição (/dev/hda, /dev/hda1...), pelo índice em blocos::listar().
    Bloco(u8),
    /// Um console (/dev/tty1..tty4).
    Console(u8),
    /// /dev/ptmx: abrir cria um pseudo-terminal.
    Ptmx,
    /// /dev/pts/N: o lado escravo de um pseudo-terminal (pelo id do terminal).
    PtyEscravo(u32),
}

/// Um nó da árvore em memória.
pub enum Conteudo {
    Arquivo(Vec<u8>),
    Diretorio(BTreeMap<String, Arc<No>>),
    Dispositivo(Dispositivo),
    /// Link simbólico: o caminho para onde aponta.
    Link(String),
    /// FIFO (pipe com nome): o pipe em si fica na tabela FIFOS.
    Fifo,
}

pub struct No {
    pub id: u64,
    pub conteudo: Mutex<Conteudo>,
    meta: Mutex<Meta>,
}

/// Dono, grupo, permissões e data de modificação.
#[derive(Clone, Copy)]
pub struct Meta {
    /// Os 12 bits baixos do modo: rwxrwxrwx + setuid, setgid e sticky.
    pub permissoes: u32,
    pub uid: u32,
    pub gid: u32,
    pub modificado: u64,
}

impl Meta {
    pub fn de_root(permissoes: u32) -> Self {
        Meta { permissoes, uid: 0, gid: 0, modificado: rtc::agora() }
    }
}

pub const SETUID: u32 = 0o4000;
pub const SETGID: u32 = 0o2000;
pub const STICKY: u32 = 0o1000;

pub const LER: u32 = 4;
pub const ESCREVER: u32 = 2;
pub const EXECUTAR: u32 = 1;

/// Quem está pedindo: o uid e o gid *efetivos* do processo.
#[derive(Clone, Copy)]
pub struct Cred {
    pub uid: u32,
    pub gid: u32,
}

/// A regra de permissões do Unix. `acesso` combina LER, ESCREVER e EXECUTAR.
pub fn permitido(meta: &Meta, cred: Cred, acesso: u32, diretorio: bool) -> bool {
    if cred.uid == 0 {
        return acesso & EXECUTAR == 0 || diretorio || meta.permissoes & 0o111 != 0;
    }
    let trio = if cred.uid == meta.uid {
        meta.permissoes >> 6
    } else if cred.gid == meta.gid {
        meta.permissoes >> 3
    } else {
        meta.permissoes
    };
    trio & acesso == acesso
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tipo {
    Arquivo,
    Diretorio,
    Dispositivo(Dispositivo),
    Link,
    Fifo,
}

/// Qualquer arquivo ou diretório, esteja na memória, no disco ou no /proc.
#[derive(Clone)]
pub enum Vnode {
    Memoria(Arc<No>),
    Disco(Arc<Ext2>, u32),
    Proc(NoProc),
}

static PROXIMO_ID: AtomicU64 = AtomicU64::new(1);
static RAIZ: Mutex<Option<Arc<No>>> = Mutex::new(None);

/// Um sistema de arquivos pendurado num diretório: quem chega a `ponto`
/// (pelo id do diretório) continua a partir de `raiz`.
struct Montagem {
    ponto: u64,
    raiz: Vnode,
    caminho: String,
    dispositivo: String,
    tipo: String,
}

static MONTAGENS: Mutex<Vec<Montagem>> = Mutex::new(Vec::new());

/// O pipe de cada FIFO em uso (pelo id do nó): os dados de um FIFO nunca
/// vão para o disco, só passam pelo kernel.
static FIFOS: Mutex<BTreeMap<u64, Arc<crate::arquivo::Pipe>>> = Mutex::new(BTreeMap::new());

/// O pipe por trás de um FIFO (criado na primeira abertura).
pub fn pipe_do_fifo(no: &Vnode) -> Arc<crate::arquivo::Pipe> {
    FIFOS.lock().entry(no.id()).or_insert_with(crate::arquivo::Pipe::vazio).clone()
}

/// Se há algo montado sobre este diretório, devolve a raiz do que está
/// montado (a última montagem por cima vale).
fn atravessar(no: Vnode) -> Vnode {
    let id = no.id();
    let montado = MONTAGENS.lock().iter().rev().find(|m| m.ponto == id).map(|m| m.raiz.clone());
    match montado {
        Some(raiz) if raiz.id() != id => atravessar(raiz),
        _ => no,
    }
}

impl No {
    fn novo(conteudo: Conteudo, meta: Meta) -> Arc<No> {
        Arc::new(No {
            id: PROXIMO_ID.fetch_add(1, Ordering::Relaxed),
            conteudo: Mutex::new(conteudo),
            meta: Mutex::new(meta),
        })
    }
}

fn meta_padrao(conteudo: &Conteudo) -> Meta {
    Meta::de_root(match conteudo {
        Conteudo::Arquivo(_) => 0o644,
        Conteudo::Dispositivo(_) => 0o666,
        Conteudo::Link(_) => 0o777,
        _ => 0o755,
    })
}

fn diretorio_vazio() -> Conteudo {
    Conteudo::Diretorio(BTreeMap::new())
}


impl Vnode {
    pub fn tipo(&self) -> Result<Tipo, i64> {
        match self {
            Vnode::Memoria(no) => Ok(match &*no.conteudo.lock() {
                Conteudo::Arquivo(_) => Tipo::Arquivo,
                Conteudo::Diretorio(_) => Tipo::Diretorio,
                Conteudo::Dispositivo(d) => Tipo::Dispositivo(*d),
                Conteudo::Link(_) => Tipo::Link,
                Conteudo::Fifo => Tipo::Fifo,
            }),
            Vnode::Disco(fs, inode) => {
                let info = fs.info(*inode)?;
                Ok(if info.diretorio {
                    Tipo::Diretorio
                } else if info.link {
                    Tipo::Link
                } else if info.fifo {
                    Tipo::Fifo
                } else {
                    Tipo::Arquivo
                })
            }
            Vnode::Proc(no) => Ok(no.tipo()),
        }
    }

    /// Número único do arquivo (o "inode" que o `stat` mostra).
    pub fn id(&self) -> u64 {
        match self {
            Vnode::Memoria(no) => no.id,
            // O número do sistema de arquivos vai nos bits de cima: dois discos
            // podem ter o mesmo número de inode.
            Vnode::Disco(fs, inode) => (fs.id as u64) << 32 | *inode as u64,
            Vnode::Proc(no) => no.id(),
        }
    }

    /// Bytes, num arquivo; quantidade de entradas, num diretório em memória.
    pub fn tamanho(&self) -> Result<u64, i64> {
        match self {
            Vnode::Memoria(no) => Ok(match &*no.conteudo.lock() {
                Conteudo::Arquivo(dados) => dados.len() as u64,
                Conteudo::Diretorio(filhos) => filhos.len() as u64,
                Conteudo::Link(alvo) => alvo.len() as u64,
                _ => 0,
            }),
            Vnode::Disco(fs, inode) => fs.info(*inode).map(|i| i.tamanho),
            Vnode::Proc(no) => match no.tipo() {
                Tipo::Diretorio => Ok(no.listar()?.len() as u64),
                Tipo::Link => Ok(no.ler_link()?.len() as u64),
                _ => Ok(no.conteudo()?.len() as u64),
            },
        }
    }

    pub fn meta(&self) -> Result<Meta, i64> {
        match self {
            Vnode::Memoria(no) => Ok(*no.meta.lock()),
            Vnode::Disco(fs, inode) => {
                let i = fs.info(*inode)?;
                Ok(Meta { permissoes: i.permissoes, uid: i.uid, gid: i.gid, modificado: i.modificado })
            }
            Vnode::Proc(no) => Ok(no.meta()),
        }
    }

    /// `chmod` e `chown`.
    pub fn alterar(&self, permissoes: Option<u32>, uid: Option<u32>, gid: Option<u32>) -> Result<(), i64> {
        match self {
            Vnode::Memoria(no) => {
                let mut m = no.meta.lock();
                if let Some(p) = permissoes {
                    m.permissoes = p & 0o7777;
                }
                m.uid = uid.unwrap_or(m.uid);
                m.gid = gid.unwrap_or(m.gid);
                Ok(())
            }
            Vnode::Disco(fs, inode) => fs.alterar(*inode, permissoes, uid, gid),
            Vnode::Proc(_) => Err(EPERM),
        }
    }

    /// Para onde um link simbólico aponta.
    pub fn ler_link(&self) -> Result<String, i64> {
        match self {
            Vnode::Memoria(no) => match &*no.conteudo.lock() {
                Conteudo::Link(alvo) => Ok(alvo.clone()),
                _ => Err(EINVAL),
            },
            Vnode::Disco(fs, inode) => fs.ler_link(*inode),
            Vnode::Proc(no) => no.ler_link(),
        }
    }

    /// O processo pode fazer `acesso` (LER/ESCREVER/EXECUTAR) neste arquivo?
    pub fn permite(&self, cred: Cred, acesso: u32) -> Result<(), i64> {
        let diretorio = self.tipo()? == Tipo::Diretorio;
        if permitido(&self.meta()?, cred, acesso, diretorio) { Ok(()) } else { Err(EACCES) }
    }

    fn filho(&self, nome: &str) -> Result<Vnode, i64> {
        match self {
            Vnode::Memoria(no) => {
                let filho = match &*no.conteudo.lock() {
                    Conteudo::Diretorio(filhos) => filhos.get(nome).cloned().ok_or(ENOENT)?,
                    _ => return Err(ENOTDIR),
                };
                Ok(Vnode::Memoria(filho))
            }
            Vnode::Disco(fs, inode) => Ok(Vnode::Disco(fs.clone(), fs.buscar(*inode, nome)?)),
            Vnode::Proc(no) => Ok(Vnode::Proc(no.filho(nome)?)),
        }
    }

    pub fn ler(&self, posicao: u64, buf: &mut [u8]) -> Result<usize, i64> {
        match self {
            Vnode::Memoria(no) => match &*no.conteudo.lock() {
                Conteudo::Arquivo(dados) => {
                    let inicio = (posicao as usize).min(dados.len());
                    let n = buf.len().min(dados.len() - inicio);
                    buf[..n].copy_from_slice(&dados[inicio..inicio + n]);
                    Ok(n)
                }
                _ => Err(EISDIR),
            },
            Vnode::Disco(fs, inode) => fs.ler(*inode, posicao, buf),
            Vnode::Proc(no) => {
                let dados = no.conteudo()?;
                let inicio = (posicao as usize).min(dados.len());
                let n = buf.len().min(dados.len() - inicio);
                buf[..n].copy_from_slice(&dados[inicio..inicio + n]);
                Ok(n)
            }
        }
    }

    pub fn escrever(&self, posicao: u64, dados: &[u8]) -> Result<usize, i64> {
        match self {
            Vnode::Memoria(no) => match &mut *no.conteudo.lock() {
                Conteudo::Arquivo(conteudo) => {
                    no.meta.lock().modificado = rtc::agora();
                    let inicio = posicao as usize;
                    let fim = inicio + dados.len();
                    if fim > conteudo.len() {
                        conteudo.resize(fim, 0);
                    }
                    conteudo[inicio..fim].copy_from_slice(dados);
                    Ok(dados.len())
                }
                _ => Err(EISDIR),
            },
            Vnode::Disco(fs, inode) => fs.escrever(*inode, posicao, dados),
            Vnode::Proc(_) => Err(EACCES),
        }
    }

    /// Esvazia o arquivo (`O_TRUNC`).
    pub fn truncar(&self) -> Result<(), i64> {
        match self {
            Vnode::Memoria(no) => {
                if let Conteudo::Arquivo(dados) = &mut *no.conteudo.lock() {
                    dados.clear();
                }
                Ok(())
            }
            Vnode::Disco(fs, inode) => fs.truncar(*inode),
            Vnode::Proc(_) => Err(EACCES),
        }
    }

    /// `truncate`: muda o tamanho do arquivo (encolhendo ou com um buraco de zeros).
    pub fn truncar_para(&self, tamanho: u64) -> Result<(), i64> {
        match self {
            Vnode::Memoria(no) => match &mut *no.conteudo.lock() {
                Conteudo::Arquivo(dados) => {
                    dados.resize(tamanho as usize, 0);
                    no.meta.lock().modificado = rtc::agora();
                    Ok(())
                }
                _ => Err(EINVAL),
            },
            Vnode::Disco(fs, inode) => fs.truncar_para(*inode, tamanho),
            Vnode::Proc(_) => Err(EACCES),
        }
    }

    /// Lê o arquivo inteiro (para o `execve`).
    pub fn ler_tudo(&self) -> Result<Vec<u8>, i64> {
        let mut dados = alloc::vec![0u8; self.tamanho()? as usize];
        let n = self.ler(0, &mut dados)?;
        dados.truncate(n);
        Ok(dados)
    }

    /// (nome, id, tipo) de cada entrada de um diretório.
    pub fn listar(&self) -> Result<Vec<(String, u64, Tipo)>, i64> {
        match self {
            Vnode::Memoria(no) => {
                let filhos: Vec<(String, Arc<No>)> = match &*no.conteudo.lock() {
                    Conteudo::Diretorio(f) => f.iter().map(|(n, no)| (n.clone(), no.clone())).collect(),
                    _ => return Err(ENOTDIR),
                };
                filhos
                    .into_iter()
                    .map(|(nome, no)| {
                        let v = Vnode::Memoria(no);
                        Ok((nome, v.id(), v.tipo()?))
                    })
                    .collect()
            }
            Vnode::Disco(fs, inode) => Ok(fs
                .listar(*inode)?
                .into_iter()
                .map(|(nome, i, tipo)| {
                    let tipo = match tipo {
                        ext2::TIPO_ENTRADA_DIRETORIO => Tipo::Diretorio,
                        ext2::TIPO_ENTRADA_LINK => Tipo::Link,
                        ext2::TIPO_ENTRADA_FIFO => Tipo::Fifo,
                        _ => Tipo::Arquivo,
                    };
                    (nome, (fs.id as u64) << 32 | i as u64, tipo)
                })
                .collect()),
            Vnode::Proc(no) => no.listar(),
        }
    }

    fn mesmo_sistema(&self, outro: &Vnode) -> bool {
        match (self, outro) {
            (Vnode::Memoria(_), Vnode::Memoria(_)) => true,
            (Vnode::Disco(a, _), Vnode::Disco(b, _)) => Arc::ptr_eq(a, b),
            (Vnode::Proc(_), Vnode::Proc(_)) => true,
            _ => false,
        }
    }
}

// -----------------------------------------------------------------------------
// Operações por caminho
// -----------------------------------------------------------------------------

pub fn init() {
    *RAIZ.lock() = Some(No::novo(diretorio_vazio(), Meta::de_root(0o755)));
    for caminho in ["/dev", "/home", "/etc", "/bin", "/proc", "/mnt"] {
        let _ = criar_na_memoria(caminho, diretorio_vazio(), None);
    }
    // Todo mundo escreve no /tmp, mas cada um só apaga o que é seu (sticky).
    let _ = criar_na_memoria("/tmp", diretorio_vazio(), Some(Meta::de_root(0o1777)));
    let _ = criar_na_memoria("/dev/tty", Conteudo::Dispositivo(Dispositivo::Terminal), None);
    for i in 0..crate::tty::CONSOLES {
        let meta = Meta::de_root(0o620);
        let _ = criar_na_memoria(&format!("/dev/tty{}", i + 1), Conteudo::Dispositivo(Dispositivo::Console(i as u8)), Some(meta));
    }
    let _ = criar_na_memoria("/dev/ptmx", Conteudo::Dispositivo(Dispositivo::Ptmx), None);
    let _ = criar_na_memoria("/dev/pts", diretorio_vazio(), None);
    let _ = criar_na_memoria("/dev/null", Conteudo::Dispositivo(Dispositivo::Nulo), None);
    let _ = criar_na_memoria("/dev/zero", Conteudo::Dispositivo(Dispositivo::Zero), None);
    let _ = criar_na_memoria("/dev/random", Conteudo::Dispositivo(Dispositivo::Aleatorio), None);
    let _ = criar_na_memoria("/dev/urandom", Conteudo::Dispositivo(Dispositivo::Aleatorio), None);
    let _ = montar("/proc", Vnode::Proc(NoProc::Raiz), "proc", "proc");
    for (i, bloco) in crate::blocos::listar().iter().enumerate() {
        let caminho = format!("/dev/{}", bloco.nome);
        let meta = Meta::de_root(0o600); // só o root lê e grava o disco cru
        let _ = criar_na_memoria(&caminho, Conteudo::Dispositivo(Dispositivo::Bloco(i as u8)), Some(meta));
    }
}

/// Troca a raiz: `nova` (um disco) passa a ser o "/", e por cima dela vão o
/// /dev e o /proc do kernel e um /tmp em memória. Os diretórios /dev, /proc
/// e /tmp precisam existir no disco (são só os pontos de montagem).
pub fn trocar_raiz(nova: Vnode, dispositivo: &str) -> Result<(), i64> {
    let memoria = Vnode::Memoria(RAIZ.lock().clone().ok_or(EINVAL)?);
    let dev = memoria.filho("dev")?;
    MONTAGENS.lock().push(Montagem {
        ponto: memoria.id(),
        raiz: nova,
        caminho: String::from("/"),
        dispositivo: String::from(dispositivo),
        tipo: String::from("ext2"),
    });
    montar("/dev", dev, "devtmpfs", "devtmpfs")?;
    montar("/proc", Vnode::Proc(NoProc::Raiz), "proc", "proc")?;
    montar("/tmp", novo_tmpfs(), "tmpfs", "tmpfs")
}

/// Um sistema de arquivos em memória novo e vazio (o `tmpfs`), para montar.
pub fn novo_tmpfs() -> Vnode {
    Vnode::Memoria(No::novo(diretorio_vazio(), Meta::de_root(0o1777)))
}

/// Pendura um sistema de arquivos (a partir da sua raiz) num diretório que
/// já existe, como o `mount` do Unix. O que havia dentro dele fica escondido
/// até a desmontagem.
pub fn montar(caminho: &str, raiz: Vnode, dispositivo: &str, tipo: &str) -> Result<(), i64> {
    let alvo = buscar(caminho)?;
    if alvo.tipo()? != Tipo::Diretorio {
        return Err(ENOTDIR);
    }
    // O mesmo disco montado duas vezes teria dois caches gravando nele: corrompe.
    if matches!(raiz, Vnode::Disco(..)) && MONTAGENS.lock().iter().any(|m| m.dispositivo == dispositivo) {
        return Err(EBUSY);
    }
    MONTAGENS.lock().push(Montagem {
        ponto: alvo.id(),
        raiz,
        caminho: String::from(caminho),
        dispositivo: String::from(dispositivo),
        tipo: String::from(tipo),
    });
    Ok(())
}

/// O `umount`: grava o que estiver pendente e tira a montagem.
pub fn desmontar(caminho: &str) -> Result<(), i64> {
    let mut m = MONTAGENS.lock();
    let i = m.iter().rposition(|x| x.caminho == caminho).ok_or(EINVAL)?;
    let dentro = format!("{}/", caminho.trim_end_matches('/'));
    if caminho == "/" || m.iter().any(|x| x.caminho.starts_with(&dentro)) {
        return Err(EBUSY); // há outra montagem lá dentro
    }
    let montagem = m.remove(i);
    drop(m);
    if let Vnode::Disco(fs, _) = &montagem.raiz {
        fs.sincronizar()?;
    }
    Ok(())
}

/// (dispositivo, ponto de montagem, tipo) — o que o /proc/mounts mostra.
pub fn montagens() -> Vec<(String, String, String)> {
    let m = MONTAGENS.lock();
    let mut lista = Vec::new();
    if !m.iter().any(|x| x.caminho == "/") {
        lista.push((String::from("rootfs"), String::from("/"), String::from("ramfs")));
    }
    // Com a raiz no disco, as montagens feitas antes (na árvore em memória) ficaram escondidas.
    let raiz_no_disco = m.iter().position(|x| x.caminho == "/").unwrap_or(0);
    let visiveis = m.iter().enumerate().filter(|(i, x)| *i >= raiz_no_disco || x.caminho == "/");
    lista.extend(visiveis.map(|(_, x)| (x.dispositivo.clone(), x.caminho.clone(), x.tipo.clone())));
    lista
}

fn sistemas_ext2() -> Vec<Arc<Ext2>> {
    MONTAGENS
        .lock()
        .iter()
        .filter_map(|m| match &m.raiz {
            Vnode::Disco(fs, _) => Some(fs.clone()),
            _ => None,
        })
        .collect()
}

/// O primeiro disco montado (para o `df` antigo).
pub fn disco() -> Option<Arc<Ext2>> {
    sistemas_ext2().into_iter().next()
}

/// O `sync`: grava tudo o que os discos montados têm pendente.
pub fn sincronizar_tudo() {
    for fs in sistemas_ext2() {
        if fs.tem_pendencias() {
            let _ = fs.sincronizar();
        }
    }
}

/// O que o `statfs` devolve: (tipo, tamanho do bloco, blocos, livres, inodes, inodes livres).
pub fn info_do_sistema(no: &Vnode) -> (u64, u64, u64, u64, u64, u64) {
    match no {
        Vnode::Disco(fs, _) => {
            let (blocos, livres, tamanho) = fs.estatisticas();
            let (inodes, inodes_livres) = fs.inodes();
            (0xEF53, tamanho, blocos, livres, inodes, inodes_livres)
        }
        Vnode::Proc(_) => (0x9FA0, 4096, 0, 0, 0, 0),
        Vnode::Memoria(_) => {
            let (total, livres) = crate::quadros::estatisticas();
            (0x0102_1994, 4096, total as u64, livres as u64, 0, 0)
        }
    }
}

/// Há algo montado neste caminho? (Pelo caminho: buscar o diretório já
/// devolveria a raiz do que está montado nele.)
fn eh_ponto_de_montagem(caminho: &str) -> bool {
    MONTAGENS.lock().iter().any(|m| m.caminho == caminho)
}

/// Como `buscar`, mas exigindo permissão de atravessar (`x`) cada diretório do caminho.
pub fn buscar_como(caminho: &str, cred: Cred) -> Result<Vnode, i64> {
    resolver(caminho, Some(cred), true)
}

/// Como `buscar_como`, mas se o último componente for um link, devolve o
/// próprio link (para `lstat`, `readlink`, `unlink`...).
pub fn buscar_como_sem_seguir(caminho: &str, cred: Cred) -> Result<Vnode, i64> {
    resolver(caminho, Some(cred), false)
}

/// Quantos links seguidos uma busca aceita antes de desistir (um link pode
/// apontar para si mesmo, direta ou indiretamente).
const MAX_SALTOS: usize = 40;

/// Anda pelo caminho, componente por componente, seguindo links simbólicos.
fn resolver(caminho: &str, cred: Option<Cred>, seguir_ultimo: bool) -> Result<Vnode, i64> {
    let mut pendentes: alloc::collections::VecDeque<String> =
        caminho.split('/').filter(|p| !p.is_empty()).map(String::from).collect();
    // A pilha de diretórios até aqui: o ".." de dentro de um link volta nela.
    let mut pilha = alloc::vec![raiz()];
    let mut saltos = 0;
    while let Some(parte) = pendentes.pop_front() {
        let atual = pilha.last().unwrap().clone();
        match parte.as_str() {
            "." => continue,
            ".." => {
                if pilha.len() > 1 {
                    pilha.pop();
                }
                continue;
            }
            _ => {}
        }
        if let Some(c) = cred {
            atual.permite(c, EXECUTAR)?;
        }
        let filho = atravessar(atual.filho(&parte)?);
        let e_o_ultimo = pendentes.is_empty();
        if filho.tipo()? == Tipo::Link && (!e_o_ultimo || seguir_ultimo) {
            saltos += 1;
            if saltos > MAX_SALTOS {
                return Err(ELOOP);
            }
            let alvo = filho.ler_link()?;
            if alvo.starts_with('/') {
                pilha.truncate(1);
            }
            for p in alvo.split('/').filter(|p| !p.is_empty()).rev() {
                pendentes.push_front(String::from(p));
            }
            continue;
        }
        pilha.push(filho);
    }
    Ok(pilha.pop().unwrap())
}

fn raiz() -> Vnode {
    atravessar(Vnode::Memoria(RAIZ.lock().clone().expect("o VFS ainda não foi iniciado")))
}

/// Junta `caminho` ao diretório `base` e resolve `.` e `..`, sem consultar o disco.
pub fn normalizar(base: &str, caminho: &str) -> String {
    let mut partes: Vec<&str> = Vec::new();
    if !caminho.starts_with('/') {
        partes.extend(base.split('/').filter(|p| !p.is_empty()));
    }
    for parte in caminho.split('/') {
        match parte {
            "" | "." => {}
            ".." => {
                partes.pop();
            }
            nome => partes.push(nome),
        }
    }
    let mut resultado = String::new();
    for parte in &partes {
        resultado.push('/');
        resultado.push_str(parte);
    }
    if resultado.is_empty() {
        resultado.push('/');
    }
    resultado
}

/// Acha um caminho (seguindo links), sem conferir permissões: uso interno do kernel.
pub fn buscar(caminho: &str) -> Result<Vnode, i64> {
    resolver(caminho, None, true)
}

pub fn buscar_sem_seguir(caminho: &str) -> Result<Vnode, i64> {
    resolver(caminho, None, false)
}

/// Cria um FIFO (o `mkfifo`).
pub fn criar_fifo(caminho: &str, meta: Meta) -> Result<(), i64> {
    let (pai, nome) = separar(caminho)?;
    match buscar(pai)? {
        Vnode::Memoria(_) => criar_na_memoria(caminho, Conteudo::Fifo, Some(meta)),
        Vnode::Disco(fs, dir) => fs.criar_fifo(dir, nome, meta.permissoes, meta.uid, meta.gid).map(|_| ()),
        Vnode::Proc(_) => Err(EACCES),
    }
}

/// Cria um link simbólico em `caminho` apontando para `alvo`.
pub fn criar_link_simbolico(caminho: &str, alvo: &str, meta: Meta) -> Result<(), i64> {
    let (pai, nome) = separar(caminho)?;
    match buscar(pai)? {
        Vnode::Memoria(_) => criar_na_memoria(caminho, Conteudo::Link(String::from(alvo)), Some(meta)),
        Vnode::Disco(fs, dir) => fs.criar_link_simbolico(dir, nome, alvo, meta.uid, meta.gid).map(|_| ()),
        Vnode::Proc(_) => Err(EACCES),
    }
}

/// Link físico: `novo` vira mais um nome para o arquivo `existente`.
pub fn criar_link(existente: &str, novo: &str) -> Result<(), i64> {
    let alvo = buscar_sem_seguir(existente)?;
    if alvo.tipo()? == Tipo::Diretorio {
        return Err(EPERM); // links para diretórios criariam ciclos na árvore
    }
    let (pai, nome) = separar(novo)?;
    let pai = buscar(pai)?;
    if !pai.mesmo_sistema(&alvo) {
        return Err(EXDEV);
    }
    match (pai, alvo) {
        (Vnode::Disco(fs, dir), Vnode::Disco(_, inode)) => fs.criar_link(dir, nome, inode),
        (Vnode::Memoria(dir), Vnode::Memoria(no)) => {
            let mut c = dir.conteudo.lock();
            let Conteudo::Diretorio(filhos) = &mut *c else { return Err(ENOTDIR) };
            if filhos.contains_key(nome) {
                return Err(EEXIST);
            }
            filhos.insert(nome.to_string(), no);
            Ok(())
        }
        _ => Err(EXDEV),
    }
}

/// "/a/b/c" → ("/a/b", "c")
pub fn separar(caminho: &str) -> Result<(&str, &str), i64> {
    let (pai, nome) = caminho.rsplit_once('/').ok_or(EINVAL)?;
    if nome.is_empty() {
        return Err(EBUSY); // a raiz
    }
    if nome.len() > 255 {
        return Err(ENAMETOOLONG);
    }
    Ok((if pai.is_empty() { "/" } else { pai }, nome))
}

/// Cria um nó diretamente na árvore em memória (usado na partida e pelo initrd).
pub fn criar_na_memoria(caminho: &str, conteudo: Conteudo, meta: Option<Meta>) -> Result<(), i64> {
    let (pai, nome) = separar(caminho)?;
    let Vnode::Memoria(pai) = buscar(pai)? else { return Err(EXDEV) };
    let mut c = pai.conteudo.lock();
    let Conteudo::Diretorio(filhos) = &mut *c else { return Err(ENOTDIR) };
    if filhos.contains_key(nome) {
        return Err(EEXIST);
    }
    let meta = meta.unwrap_or_else(|| meta_padrao(&conteudo));
    filhos.insert(nome.to_string(), No::novo(conteudo, meta));
    Ok(())
}

/// Como `mkdir -p`, só na memória.
pub fn criar_diretorios(caminho: &str) {
    let mut atual = String::new();
    for parte in caminho.split('/').filter(|p| !p.is_empty()) {
        atual.push('/');
        atual.push_str(parte);
        let _ = criar_na_memoria(&atual, diretorio_vazio(), None);
    }
}

/// Cria um arquivo vazio ou um diretório, onde quer que o pai esteja.
pub fn criar(caminho: &str, diretorio: bool, meta: Meta) -> Result<Vnode, i64> {
    let (pai, nome) = separar(caminho)?;
    match buscar(pai)? {
        Vnode::Memoria(_) => {
            let conteudo = if diretorio { diretorio_vazio() } else { Conteudo::Arquivo(Vec::new()) };
            criar_na_memoria(caminho, conteudo, Some(meta))?;
            buscar(caminho)
        }
        Vnode::Disco(fs, dir) => {
            let inode = fs.criar(dir, nome, diretorio, meta.permissoes, meta.uid, meta.gid)?;
            Ok(Vnode::Disco(fs, inode))
        }
        Vnode::Proc(_) => Err(EACCES),
    }
}

/// Apaga um arquivo (`unlink`) ou um diretório vazio (`rmdir`).
pub fn remover(caminho: &str, diretorio: bool) -> Result<(), i64> {
    let (pai, nome) = separar(caminho)?;
    if eh_ponto_de_montagem(caminho) {
        return Err(EBUSY);
    }
    match buscar(pai)? {
        Vnode::Proc(_) => Err(EACCES),
        Vnode::Disco(fs, dir) => fs.remover(dir, nome, diretorio),
        Vnode::Memoria(pai) => {
            let mut c = pai.conteudo.lock();
            let Conteudo::Diretorio(filhos) = &mut *c else { return Err(ENOTDIR) };
            let problema = match (&*filhos.get(nome).ok_or(ENOENT)?.conteudo.lock(), diretorio) {
                (Conteudo::Diretorio(f), true) if !f.is_empty() => Some(ENOTEMPTY),
                (Conteudo::Diretorio(_), true) => None,
                (Conteudo::Diretorio(_), false) => Some(EISDIR),
                (_, true) => Some(ENOTDIR),
                (_, false) => None,
            };
            if let Some(erro) = problema {
                return Err(erro);
            }
            filhos.remove(nome);
            Ok(())
        }
    }
}

/// Move/renomeia dentro do mesmo sistema de arquivos. Um arquivo no destino é substituído.
pub fn renomear(de: &str, para: &str) -> Result<(), i64> {
    if para.starts_with(de) && para.as_bytes().get(de.len()) == Some(&b'/') {
        return Err(EINVAL); // mover um diretório para dentro dele mesmo
    }
    if eh_ponto_de_montagem(de) || eh_ponto_de_montagem(para) {
        return Err(EBUSY);
    }
    let (pai_de, nome_de) = separar(de)?;
    let (pai_para, nome_para) = separar(para)?;
    let (pai_de, pai_para) = (buscar(pai_de)?, buscar(pai_para)?);
    if !pai_de.mesmo_sistema(&pai_para) {
        return Err(EXDEV); // entre memória e disco: o `mv` teria que copiar
    }
    match (pai_de, pai_para) {
        (Vnode::Disco(fs, a), Vnode::Disco(_, b)) => fs.renomear(a, nome_de, b, nome_para),
        (Vnode::Memoria(a), Vnode::Memoria(b)) => {
            if let Ok(existente) = Vnode::Memoria(b.clone()).filho(nome_para) {
                if existente.tipo()? == Tipo::Diretorio {
                    return Err(EISDIR);
                }
            }
            let no = {
                let mut c = a.conteudo.lock();
                let Conteudo::Diretorio(filhos) = &mut *c else { return Err(ENOTDIR) };
                filhos.remove(nome_de).ok_or(ENOENT)?
            };
            match &mut *b.conteudo.lock() {
                Conteudo::Diretorio(filhos) => {
                    filhos.insert(nome_para.to_string(), no);
                    Ok(())
                }
                _ => Err(ENOTDIR),
            }
        }
        _ => Err(EXDEV),
    }
}
