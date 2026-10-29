//! Chamadas de sistema: a porta de entrada dos programas para o kernel.
//!
//! O programa põe o número da chamada em RAX e os argumentos em RDI, RSI,
//! RDX, R10, R8 e R9, e executa a instrução `syscall` (ou `int 0x80`). A CPU
//! sobe para o anel 0 e cai em `entrada_syscall` (interrupcoes.asm), que monta
//! o mesmo `Contexto` de uma interrupção. A resposta volta em RAX; um erro é
//! um número negativo (-ENOENT...).
//!
//! Os números e o formato das estruturas são os mesmos do Linux x86_64.

use crate::arquivo::{Arquivo, POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, Pipe, Tipo};
use crate::erro::*;
use crate::interrupcoes::Contexto;
use crate::paginacao::EspacoEnderecos;
use crate::paginacao::{Fonte, Regiao};
use crate::processo::{self, MAX_ARQUIVOS};
use crate::vfs::{self, Vnode};
use crate::{epoll, heap, pit, porta, println, quadros, rtc, sinal, socket, tty};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;

const EFER: u32 = 0xC000_0080;
const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const SFMASK: u32 = 0xC000_0084;

unsafe extern "C" {
    fn entrada_syscall();
}

pub fn ler_msr(msr: u32) -> u64 {
    let (baixo, alto): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") baixo, out("edx") alto, options(nomem, nostack)) };
    ((alto as u64) << 32) | baixo as u64
}

pub fn escrever_msr(msr: u32, valor: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") valor as u32, in("edx") (valor >> 32) as u32, options(nostack))
    };
}

/// Liga a instrução `syscall` e diz para onde ela salta.
pub fn init() {
    escrever_msr(EFER, ler_msr(EFER) | 1); // SCE: System Call Extensions
    // Seletores: kernel em 0x08 (código) / 0x10 (pilha); usuário a partir de 0x10.
    escrever_msr(STAR, (0x10 << 48) | (0x08 << 32));
    escrever_msr(LSTAR, entrada_syscall as *const () as u64);
    // Bits de RFLAGS zerados ao entrar: IF (interrupções), DF e TF.
    escrever_msr(SFMASK, (1 << 9) | (1 << 10) | (1 << 8));
}

pub(crate) type Resultado = Result<u64, i64>;

pub fn tratar(ctx: &mut Contexto) {
    // ptrace: na entrada, o rastreador vê o número em orig_rax (e pode trocá-lo).
    *processo::syscall_em_andamento() = ctx.rax;
    if processo::esta_rastreado() {
        ctx.rax = (-ENOSYS) as u64;
        processo::parada_de_syscall();
    }
    let numero = *processo::syscall_em_andamento();
    let (a1, a2, a3, a4, a5) = (ctx.rdi, ctx.rsi, ctx.rdx, ctx.r10, ctx.r8);
    let resultado = match numero {
        0 => sys_read(a1, a2, a3),
        1 => sys_write(a1, a2, a3),
        2 => sys_open(a1, a2, a3),
        // lstat: não segue o link (a não ser com uma barra no fim, como "link/")
        6 => caminho(a1).and_then(|c| stat_caminho(&c, a2, termina_em_barra(a1))),
        7 => sys_poll(a1, a2, if a3 as i32 >= 0 { Some(a3 as i32 as u64) } else { None }),
        9 => sys_mmap(a1, a2, a3, a4, a5, ctx.r9),
        10 => sys_mprotect(a1, a2, a3),
        11 => sys_munmap(a1, a2),
        19 => sys_vetorial(a1, a2, a3, false),
        20 => sys_vetorial(a1, a2, a3, true),
        21 => sys_access(caminho(a1), a2),
        23 => prazo_de_timeval(a5).and_then(|prazo| sys_select(a1, a2, a3, a4, prazo)),
        24 => {
            processo::escalonar(); // sched_yield: cede a vez
            Ok(0)
        }
        26 => sys_msync(a1, a2),
        28 => Ok(0), // madvise: só um conselho; pode ser ignorado
        41 => socket::socket(a1, a2),
        42 => socket::connect(a1, a2, a3),
        43 => socket::accept(a1, a2, a3, 0),
        288 => socket::accept(a1, a2, a3, a4),
        44 => socket::sendto(a1, a2, a3, a4, a5, ctx.r9),
        45 => socket::recvfrom(a1, a2, a3, a4, a5, ctx.r9),
        46 => socket::sendmsg(a1, a2, a3),
        47 => socket::recvmsg(a1, a2, a3),
        48 => socket::shutdown(a1, a2),
        49 => socket::bind(a1, a2, a3),
        50 => socket::listen(a1, a2),
        51 => socket::getsockname(a1, a2, a3),
        52 => socket::getpeername(a1, a2, a3),
        53 => socket::socketpair(a1, a2, a4),
        54 => Ok(0), // setsockopt: as opções (tamanhos de buffer etc.) são aceitas e ignoradas
        55 => socket::getsockopt(a1, a2, a3, a4, a5),
        213 | 291 => epoll::criar(if numero == 291 { a1 } else { 0 }),
        233 => epoll::controlar(a1, a2, a3, a4),
        232 => epoll::esperar(a1, a2, a3, if a4 as i32 >= 0 { Some(a4 as i32 as u64) } else { None }),
        281 => epoll::esperar(a1, a2, a3, if a4 as i32 >= 0 { Some(a4 as i32 as u64) } else { None }), // epoll_pwait (sem máscara)
        103 => crate::registro::syscall(a1, a2, a3), // syslog: o registro do kernel (dmesg)
        29 => crate::shm::shmget(a1 as i32, a2, a3),
        30 => crate::shm::shmat(a1 as i32, a2, a3),
        31 => crate::shm::shmctl(a1 as i32, a2, a3),
        67 => crate::shm::shmdt(a1),
        3 => sys_close(a1),
        4 => sys_stat(a1, a2),
        5 => sys_fstat(a1, a2),
        8 => arquivo(a1).and_then(|f| f.posicionar(a2 as i64, a3)),
        12 => sys_brk(a1),
        16 => sys_ioctl(a1, a2, a3),
        13 => sys_sigaction(a1, a2, a3),
        14 => sys_sigprocmask(a1, a2, a3),
        15 => Ok(processo::retornar_de_sinal(ctx)),
        22 => sys_pipe(a1, 0),
        32 => arquivo(a1).and_then(novo_descritor),
        33 => sys_dup2(a1, a2, 0),
        34 => sys_pause(),
        35 => sys_nanosleep(a1),
        37 => sys_alarm(a1),
        39 => Ok(processo::atual().tgid as u64),
        56 => sys_clone(ctx, a1, a2, a3, a4, a5),
        57 | 58 => processo::fork(ctx).map(|pid| pid as u64), // vfork vira fork
        59 => sys_execve(ctx, a1, a2, a3),
        60 => processo::sair_da_thread(((a1 & 0xFF) << 8) as i32),
        231 => processo::sair(((a1 & 0xFF) << 8) as i32),
        61 => sys_wait4(a1, a2, a3),
        62 => processo::sinalizar(a1 as i32 as i64, a2 as u32).map(|_| 0),
        63 => sys_uname(a1),
        72 => sys_fcntl(a1, a2, a3),
        74 | 75 => sys_fsync(a1), // fsync/fdatasync
        76 => caminho(a1).and_then(|c| sys_truncate(&c, a2)),
        77 => sys_ftruncate(a1, a2),
        86 => sys_link(caminho(a1), caminho(a2)),
        88 => sys_symlink(a1, caminho(a2)),
        133 => caminho(a1).and_then(|c| sys_mknod(c, a2)),
        89 => sys_readlink(caminho(a1), a2, a3),
        96 => sys_gettimeofday(a1),
        79 => sys_getcwd(a1, a2),
        80 => sys_chdir(a1),
        82 => sys_rename(a1, a2),
        83 => caminho(a1).and_then(|c| mkdir_caminho(c, a2)),
        84 => caminho(a1).and_then(|c| remover_caminho(c, true)),
        87 => caminho(a1).and_then(|c| remover_caminho(c, false)),
        90 => sys_chmod(a1, a2),
        92 => sys_chown(a1, a2, a3),
        95 => sys_umask(a1),
        102 => Ok(processo::atual().uid as u64),
        104 => Ok(processo::atual().gid as u64),
        105 => sys_setuid(a1 as u32),
        106 => sys_setgid(a1 as u32),
        107 => Ok(processo::atual().euid as u64),
        108 => Ok(processo::atual().egid as u64),
        109 => processo::definir_grupo(a1 as u32, a2 as u32).map(|_| 0),
        110 => Ok(processo::atual().pai as u64),
        111 => Ok(processo::atual().pgid as u64),
        112 => processo::nova_sessao().map(|s| s as u64),
        121 => processo::grupo_e_sessao(a1 as u32).map(|(g, _)| g as u64),
        124 => processo::grupo_e_sessao(a1 as u32).map(|(_, s)| s as u64),
        137 => caminho(a1).and_then(|c| vfs::buscar_como(&c, cred())).and_then(|no| sys_statfs(&no, a2)),
        138 => arquivo(a1).and_then(|arq| match &arq.tipo {
            Tipo::No(no) => sys_statfs(no, a2),
            _ => Err(EINVAL),
        }),
        158 => sys_arch_prctl(a1, a2),
        162 | 306 => {
            vfs::sincronizar_tudo(); // sync, syncfs
            Ok(0)
        }
        165 => sys_mount(a1, a2, a3),
        166 => sys_umount(a1),
        186 => Ok(processo::atual().pid as u64), // gettid
        200 => processo::enviar_a_thread(0, a1 as u32, a2 as u32).map(|_| 0), // tkill
        101 => processo::ptrace(a1, a2 as u32, a3, a4),
        202 => sys_futex(a1, a2, a3, a4, a5, ctx.r9),
        201 => sys_time(a1),
        218 => {
            // set_tid_address: onde zerar o tid quando esta thread terminar
            processo::atual().limpar_tid = a1;
            Ok(processo::atual().pid as u64)
        }
        230 => sys_nanosleep(a3), // clock_nanosleep (sem TIMER_ABSTIME)
        257 => caminho_at(a1, a2).and_then(|c| abrir_caminho(c, a3, ctx.r10)),
        258 => caminho_at(a1, a2).and_then(|c| mkdir_caminho(c, a3)),
        234 => processo::pode_sinalizar(a1 as u32).and_then(|_| processo::enviar_a_thread(a1 as u32, a2 as u32, a3 as u32)).map(|_| 0), // tgkill
        262 => sys_newfstatat(a1, a2, a3, ctx.r10),
        270 => prazo_de_timespec(a5).and_then(|prazo| sys_select(a1, a2, a3, a4, prazo)), // pselect6 (sem máscara)
        271 => prazo_de_timespec(a3).and_then(|prazo| sys_poll(a1, a2, prazo)), // ppoll (sem máscara)
        259 => caminho_at(a1, a2).and_then(|c| sys_mknod(c, a3)),
        265 => sys_link(caminho_at(a1, a2), caminho_at(a3, ctx.r10)),
        266 => sys_symlink(a1, caminho_at(a2, a3)),
        267 => sys_readlink(caminho_at(a1, a2), a3, ctx.r10),
        263 => caminho_at(a1, a2).and_then(|c| remover_caminho(c, a3 & 0x200 != 0)),
        264 => sys_renameat(a1, a2, a3, ctx.r10),
        269 | 439 => sys_access(caminho_at(a1, a2), a3),
        292 => sys_dup2(a1, a2, a3),
        293 => sys_pipe(a1, a2),
        302 => Err(ENOSYS), // prlimit64: sem limites de recursos
        318 => sys_getrandom(a1, a2),
        169 => sys_reboot(a3),
        217 => sys_getdents64(a1, a2, a3),
        228 => sys_clock_gettime(a1, a2),
        // Extensões do FerrugemOS (o Linux usaria /proc para isso)
        1000 => sys_processos(a1, a2),
        1001 => sys_memoria(a1),
        1002 => sys_disco(a1),
        n => {
            println!("[kernel] {}: chamada de sistema {} não implementada", processo::atual().nome, n);
            Err(ENOSYS)
        }
    };
    // Interrompida por um sinal cujo tratador pediu SA_RESTART: volta o RIP
    // para a instrução `syscall` (2 bytes), e ela roda de novo depois do tratador.
    if resultado == Err(ERESTARTSYS) || resultado == Err(EINTR) && processo::reiniciar_syscall() {
        ctx.rax = numero;
        ctx.rip -= 2;
        return;
    }
    ctx.rax = match resultado {
        Ok(valor) => valor,
        Err(erro) => (-erro) as u64,
    };
    if processo::esta_rastreado() {
        processo::parada_de_syscall(); // e na saída, com o resultado em rax
    }
}

// -----------------------------------------------------------------------------
// Acesso à memória do programa
// -----------------------------------------------------------------------------

fn espaco() -> &'static EspacoEnderecos {
    processo::atual().espaco().expect("syscall sem espaço de usuário")
}

pub(crate) fn ler_usuario(ptr: u64, tamanho: usize) -> Result<Vec<u8>, i64> {
    espaco().copiar_de(ptr, tamanho).ok_or(EFAULT)
}

pub(crate) fn escrever_usuario(ptr: u64, dados: &[u8]) -> Result<(), i64> {
    if espaco().copiar_para(ptr, dados) { Ok(()) } else { Err(EFAULT) }
}

/// Lê um caminho do programa e já o resolve em relação ao diretório atual.
pub(crate) fn caminho(ptr: u64) -> Result<String, i64> {
    let texto = espaco().ler_string(ptr, 4096).ok_or(EFAULT)?;
    if texto.is_empty() {
        return Err(ENOENT);
    }
    Ok(vfs::normalizar(&processo::atual().fs().diretorio, &texto))
}

/// "dir/" obriga a seguir um link no fim do caminho (e exige que dê num diretório).
fn termina_em_barra(ptr: u64) -> bool {
    espaco().ler_string(ptr, 4096).is_some_and(|s| s.len() > 1 && s.ends_with('/'))
}

pub(crate) fn arquivo(fd: u64) -> Result<Arc<Arquivo>, i64> {
    processo::atual()
        .arquivos()
        .lista
        .get(fd as usize)
        .and_then(|a| a.clone())
        .ok_or(EBADF)
}

/// Põe o arquivo no menor descritor livre (regra do Unix: por isso `close(1)`
/// seguido de `open` faz o arquivo virar a saída padrão).
pub(crate) fn novo_descritor(arq: Arc<Arquivo>) -> Resultado {
    let tabela = &mut processo::atual().arquivos().lista;
    if let Some(fd) = tabela.iter().position(|a| a.is_none()) {
        tabela[fd] = Some(arq);
        return Ok(fd as u64);
    }
    if tabela.len() >= MAX_ARQUIVOS {
        return Err(EMFILE);
    }
    tabela.push(Some(arq));
    Ok(tabela.len() as u64 - 1)
}

// -----------------------------------------------------------------------------
// Arquivos
// -----------------------------------------------------------------------------

fn sys_read(fd: u64, buf: u64, tamanho: u64) -> Resultado {
    let arq = arquivo(fd)?;
    let mut dados = vec![0u8; (tamanho as usize).min(64 * 1024)];
    let lidos = arq.ler(&mut dados)?;
    escrever_usuario(buf, &dados[..lidos])?;
    Ok(lidos as u64)
}

fn sys_write(fd: u64, buf: u64, tamanho: u64) -> Resultado {
    let arq = arquivo(fd)?;
    let dados = ler_usuario(buf, (tamanho as usize).min(1024 * 1024))?;
    arq.escrever(&dados).map(|n| n as u64)
}

pub(crate) fn cred() -> vfs::Cred {
    processo::atual().cred()
}

/// Permissões de um arquivo novo: as pedidas, menos as da umask.
fn permissoes_novas(pedidas: u64) -> u32 {
    pedidas as u32 & 0o7777 & !processo::atual().fs().umask
}

const O_CLOEXEC: u64 = 0o2000000;
const AT_FDCWD: u64 = -100i64 as u64;

fn sys_open(ptr: u64, flags: u64, permissoes: u64) -> Resultado {
    abrir_caminho(caminho(ptr)?, flags, permissoes)
}

fn abrir_caminho(caminho: String, flags: u64, permissoes: u64) -> Resultado {
    let fd = novo_descritor(Arquivo::abrir(&caminho, flags, cred(), permissoes_novas(permissoes))?)?;
    if flags & O_CLOEXEC != 0 {
        processo::atual().arquivos().fecha_no_exec |= 1 << fd;
    }
    Ok(fd)
}

/// As chamadas `*at` (openat, mkdirat...): o caminho é relativo a `dirfd`.
/// Aceitamos AT_FDCWD (o diretório atual) ou um caminho absoluto, o que cobre
/// o que a libc faz por conta própria.
fn caminho_at(dirfd: u64, ptr: u64) -> Result<String, i64> {
    let texto = espaco().ler_string(ptr, 4096).ok_or(EFAULT)?;
    if dirfd != AT_FDCWD && !texto.starts_with('/') {
        return Err(EINVAL);
    }
    caminho(ptr)
}

/// O diretório onde `caminho` está, conferindo que podemos escrever nele.
pub(crate) fn pai_gravavel(caminho: &str) -> Result<Vnode, i64> {
    let (pai, _) = vfs::separar(caminho)?;
    let pai = vfs::buscar_como(pai, cred())?;
    pai.permite(cred(), vfs::ESCREVER | vfs::EXECUTAR)?;
    Ok(pai)
}

/// Pode apagar (ou mover) `caminho`? Precisa poder escrever no diretório e,
/// se ele tiver o sticky bit (o /tmp), ser dono do arquivo ou do diretório.
fn pode_apagar(caminho: &str) -> Result<(), i64> {
    let pai = pai_gravavel(caminho)?;
    let alvo = vfs::buscar_sem_seguir(caminho)?; // apagar um link apaga o link, não o alvo
    let (c, mp, ma) = (cred(), pai.meta()?, alvo.meta()?);
    if mp.permissoes & vfs::STICKY != 0 && c.uid != 0 && c.uid != ma.uid && c.uid != mp.uid {
        return Err(EPERM);
    }
    Ok(())
}

fn mkdir_caminho(caminho: String, permissoes: u64) -> Resultado {
    pai_gravavel(&caminho)?;
    let c = cred();
    let meta = vfs::Meta { permissoes: permissoes_novas(permissoes), uid: c.uid, gid: c.gid, modificado: rtc::agora() };
    vfs::criar(&caminho, true, meta).map(|_| 0)
}

fn remover_caminho(caminho: String, diretorio: bool) -> Resultado {
    pode_apagar(&caminho)?;
    vfs::remover(&caminho, diretorio).map(|_| 0)
}

/// Só o dono (ou o root) muda as permissões.
fn sys_chmod(ptr: u64, permissoes: u64) -> Resultado {
    let no = vfs::buscar_como(&caminho(ptr)?, cred())?;
    let c = cred();
    if c.uid != 0 && c.uid != no.meta()?.uid {
        return Err(EPERM);
    }
    no.alterar(Some(permissoes as u32), None, None).map(|_| 0)
}

/// Só o root muda o dono (senão daria para "dar" arquivos e fugir de cotas).
/// -1 mantém o valor atual.
fn sys_chown(ptr: u64, uid: u64, gid: u64) -> Resultado {
    let no = vfs::buscar_como(&caminho(ptr)?, cred())?;
    if cred().uid != 0 {
        return Err(EPERM);
    }
    let manter = |v: u64| if v as u32 == u32::MAX { None } else { Some(v as u32) };
    no.alterar(None, manter(uid), manter(gid)).map(|_| 0)
}

fn sys_umask(nova: u64) -> Resultado {
    let fs = processo::atual().fs();
    let antiga = fs.umask;
    fs.umask = nova as u32 & 0o777;
    Ok(antiga as u64)
}

/// O root vira quem quiser (é o que o `login` faz). Os outros só podem
/// voltar para o próprio uid real (um programa setuid "devolvendo" o poder).
fn sys_setuid(uid: u32) -> Resultado {
    let p = processo::atual();
    if p.euid == 0 {
        (p.uid, p.euid) = (uid, uid);
    } else if uid == p.uid {
        p.euid = uid;
    } else {
        return Err(EPERM);
    }
    Ok(0)
}

fn sys_setgid(gid: u32) -> Resultado {
    let p = processo::atual();
    if p.euid == 0 {
        (p.gid, p.egid) = (gid, gid);
    } else if gid == p.gid {
        p.egid = gid;
    } else {
        return Err(EPERM);
    }
    Ok(0)
}

/// `ioctl`: só existem os de terminal (termios, tamanho da janela).
fn sys_ioctl(fd: u64, pedido: u64, ptr: u64) -> Resultado {
    let arq = arquivo(fd)?;
    let Some((terminal, mestre)) = arq.terminal() else { return Err(ENOTTY) };
    let dados = match tty::bytes_de_entrada(pedido) {
        0 => None,
        n => Some(ler_usuario(ptr, n)?),
    };
    let resposta = tty::ioctl(terminal, pedido, dados.as_deref(), mestre)?;
    if !resposta.is_empty() {
        escrever_usuario(ptr, &resposta)?;
    }
    Ok(0)
}

fn sys_close(fd: u64) -> Resultado {
    let a = processo::atual().arquivos();
    let entrada = a.lista.get_mut(fd as usize).ok_or(EBADF)?;
    entrada.take().ok_or(EBADF)?;
    a.fecha_no_exec &= !(1 << fd);
    Ok(0)
}

/// `dup2`/`dup3`: `novo` passa a apontar para o mesmo arquivo que `antigo`.
fn sys_dup2(antigo: u64, novo: u64, flags: u64) -> Resultado {
    let arq = arquivo(antigo)?;
    let novo = novo as usize;
    if novo >= MAX_ARQUIVOS {
        return Err(EBADF);
    }
    let a = processo::atual().arquivos();
    if a.lista.len() <= novo {
        a.lista.resize(novo + 1, None);
    }
    a.lista[novo] = Some(arq);
    a.fecha_no_exec &= !(1 << novo);
    if flags & O_CLOEXEC != 0 {
        a.fecha_no_exec |= 1 << novo;
    }
    Ok(novo as u64)
}

fn sys_fcntl(fd: u64, comando: u64, argumento: u64) -> Resultado {
    const F_DUPFD: u64 = 0;
    const F_GETFD: u64 = 1;
    const F_SETFD: u64 = 2;
    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;
    const F_DUPFD_CLOEXEC: u64 = 1030;
    let arq = arquivo(fd)?;
    let p = processo::atual().arquivos();
    match comando {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let livre = (argumento as usize..MAX_ARQUIVOS)
                .find(|&i| p.lista.get(i).is_none_or(|a| a.is_none()))
                .ok_or(EMFILE)?;
            let flags = if comando == F_DUPFD_CLOEXEC { O_CLOEXEC } else { 0 };
            sys_dup2(fd, livre as u64, flags)
        }
        F_GETFD => Ok((p.fecha_no_exec >> fd) & 1),
        F_SETFD => {
            if argumento & 1 != 0 {
                p.fecha_no_exec |= 1 << fd;
            } else {
                p.fecha_no_exec &= !(1 << fd);
            }
            Ok(0)
        }
        F_GETFL => Ok(arq.flags() & (0o3 | crate::arquivo::O_APPEND | crate::arquivo::O_NONBLOCK)),
        F_SETFL => {
            arq.definir_flags(argumento);
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// `readv`/`writev`: vários pedaços de memória numa chamada só.
/// Cada `iovec` tem 16 bytes: endereço e tamanho.
fn sys_vetorial(fd: u64, iov: u64, quantos: u64, escrita: bool) -> Resultado {
    if quantos > 1024 {
        return Err(EINVAL);
    }
    let mut total = 0;
    for i in 0..quantos {
        let par = ler_usuario(iov + i * 16, 16)?;
        let base = u64::from_le_bytes(par[..8].try_into().unwrap());
        let tamanho = u64::from_le_bytes(par[8..].try_into().unwrap());
        if tamanho == 0 {
            continue;
        }
        let n = if escrita { sys_write(fd, base, tamanho)? } else { sys_read(fd, base, tamanho)? };
        total += n;
        if n < tamanho {
            break; // leitura curta: não continua nos próximos pedaços
        }
    }
    Ok(total)
}

fn sys_access(caminho: Result<String, i64>, modo: u64) -> Resultado {
    let no = vfs::buscar_como(&caminho?, cred())?;
    if modo & 7 != 0 {
        no.permite(cred(), modo as u32 & 7)?;
    }
    Ok(0)
}

fn sys_newfstatat(dirfd: u64, ptr: u64, buf: u64, flags: u64) -> Resultado {
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    const AT_EMPTY_PATH: u64 = 0x1000;
    let vazio = espaco().ler_string(ptr, 1).is_some_and(|s| s.is_empty());
    if flags & AT_EMPTY_PATH != 0 && vazio {
        return sys_fstat(dirfd, buf);
    }
    stat_caminho(&caminho_at(dirfd, ptr)?, buf, flags & AT_SYMLINK_NOFOLLOW == 0 || termina_em_barra(ptr))
}

fn sys_renameat(dir_de: u64, de: u64, dir_para: u64, para: u64) -> Resultado {
    let (de, para) = (caminho_at(dir_de, de)?, caminho_at(dir_para, para)?);
    pode_apagar(&de)?;
    pai_gravavel(&para)?;
    vfs::renomear(&de, &para).map(|_| 0)
}

/// `ftruncate`: precisa ter aberto o arquivo para escrever.
fn sys_ftruncate(fd: u64, tamanho: u64) -> Resultado {
    let arq = arquivo(fd)?;
    if arq.flags() & 3 == 0 {
        return Err(EINVAL);
    }
    let Tipo::No(no) = &arq.tipo else { return Err(EINVAL) };
    no.truncar_para(tamanho).map(|_| 0)
}

fn sys_truncate(caminho: &str, tamanho: u64) -> Resultado {
    let no = vfs::buscar_como(caminho, cred())?;
    no.permite(cred(), vfs::ESCREVER)?;
    no.truncar_para(tamanho).map(|_| 0)
}

/// `mknod`: aqui, só FIFOs (S_IFIFO) e arquivos comuns. Dispositivos não.
fn sys_mknod(caminho: String, modo: u64) -> Resultado {
    pai_gravavel(&caminho)?;
    let c = cred();
    let meta = vfs::Meta { permissoes: permissoes_novas(modo), uid: c.uid, gid: c.gid, modificado: rtc::agora() };
    match modo as u32 & 0o170000 {
        S_IFIFO => vfs::criar_fifo(&caminho, meta).map(|_| 0),
        0 | S_IFREG => vfs::criar(&caminho, false, meta).map(|_| 0),
        _ => Err(EPERM),
    }
}

/// `mmap`: uma região nova na memória do processo, zerada (MAP_ANONYMOUS)
/// ou com o conteúdo de um arquivo. As páginas só existem quando tocadas;
/// as de MAP_SHARED são alocadas na hora (para um fork poder compartilhá-las).
fn sys_mmap(endereco: u64, tamanho: u64, prot: u64, flags: u64, fd: u64, deslocamento: u64) -> Resultado {
    const MAP_SHARED: u64 = 0x01;
    const MAP_FIXED: u64 = 0x10;
    const MAP_ANONYMOUS: u64 = 0x20;
        if tamanho == 0 || tamanho > 1 << 34 || deslocamento & 0xFFF != 0 {
        return Err(EINVAL);
    }
    let tamanho = (tamanho + 0xFFF) & !0xFFF;
    let compartilhada = flags & MAP_SHARED != 0;
    let fonte = if flags & MAP_ANONYMOUS != 0 && compartilhada {
        // Páginas que o pai e os filhos (depois do fork) dividem de verdade.
        Fonte::Segmento { segmento: crate::paginacao::Segmento::novo(tamanho), pagina: 0 }
    } else if flags & MAP_ANONYMOUS != 0 {
        Fonte::Anonima
    } else {
        let arq = arquivo(fd)?;
        let Tipo::No(no) = &arq.tipo else { return Err(ENODEV) };
        if no.tipo()? != vfs::Tipo::Arquivo {
            return Err(ENODEV);
        }
        let acesso = arq.flags() & 3;
        if acesso == 1 || compartilhada && prot & 2 != 0 && acesso != 2 {
            return Err(EACCES); // precisa ter aberto para ler (e para escrever, se for gravar de volta)
        }
        let total = no.tamanho()?;
        Fonte::Arquivo { no: no.clone(), deslocamento, tamanho: total.saturating_sub(deslocamento) }
    };
    let p = processo::atual();
    let inicio = if flags & MAP_FIXED != 0 {
        if endereco & 0xFFF != 0 || endereco < crate::paginacao::USUARIO_INICIO {
            return Err(EINVAL);
        }
        endereco
    } else {
        let m = p.mem().ok_or(EINVAL)?;
        let inicio = m.mmap_topo - tamanho;
        if inicio < m.brk + 0x1000_0000 {
            return Err(ENOMEM); // encostaria no heap
        }
        m.mmap_topo = inicio;
        inicio
    };
    let regiao = Regiao { inicio, fim: inicio + tamanho, prot: prot as u8 & 7, compartilhada, fonte };
    let espaco = p.espaco().ok_or(EINVAL)?;
    if compartilhada && matches!(regiao.fonte, Fonte::Arquivo { .. }) {
        if !espaco.criar_regiao_ja_alocada(regiao) {
            espaco.remover_faixa(inicio, inicio + tamanho);
            return Err(ENOMEM);
        }
    } else {
        espaco.criar_regiao(regiao);
    }
    Ok(inicio)
}

fn faixa_valida(endereco: u64, tamanho: u64) -> Result<(u64, u64), i64> {
    if endereco & 0xFFF != 0 || endereco < crate::paginacao::USUARIO_INICIO || tamanho > 1 << 40 {
        return Err(EINVAL);
    }
    Ok((endereco, endereco + ((tamanho + 0xFFF) & !0xFFF)))
}

fn sys_munmap(endereco: u64, tamanho: u64) -> Resultado {
    let (inicio, fim) = faixa_valida(endereco, tamanho)?;
    processo::atual().espaco().ok_or(EINVAL)?.remover_faixa(inicio, fim);
    Ok(0)
}

fn sys_mprotect(endereco: u64, tamanho: u64, prot: u64) -> Resultado {
    let (inicio, fim) = faixa_valida(endereco, tamanho)?;
    let espaco = processo::atual().espaco().ok_or(EINVAL)?;
    if espaco.proteger(inicio, fim, prot as u8 & 7) { Ok(0) } else { Err(ENOMEM) }
}

/// `msync`: grava agora, no arquivo, o que foi escrito num mapeamento MAP_SHARED.
fn sys_msync(endereco: u64, tamanho: u64) -> Resultado {
    let (inicio, fim) = faixa_valida(endereco, tamanho)?;
    processo::atual().espaco().ok_or(EINVAL)?.sincronizar(inicio, fim);
    Ok(0)
}

/// `getrandom`: da RDRAND, se a CPU tiver (veja aleatorio.rs).
fn sys_getrandom(buf: u64, tamanho: u64) -> Resultado {
    let mut dados = vec![0u8; (tamanho as usize).min(4096)];
    crate::aleatorio::preencher(&mut dados);
    escrever_usuario(buf, &dados)?;
    Ok(dados.len() as u64)
}

// -----------------------------------------------------------------------------
// poll e select: esperar vários descritores ao mesmo tempo
// -----------------------------------------------------------------------------

/// O coração do poll/select: confere todos os descritores e, se nenhum estiver
/// pronto, dorme até um evento de E/S (ou até o prazo). `prazo_ms`: None espera
/// para sempre; Some(0) só confere.
fn esperar_descritores(pedidos: &[(i32, u16)], prazo_ms: Option<u64>) -> Result<Vec<u16>, i64> {
    let limite = prazo_ms.map(|ms| pit::milissegundos() + ms);
    loop {
        let respostas: Vec<u16> = pedidos
            .iter()
            .map(|&(fd, eventos)| {
                if fd < 0 {
                    return 0;
                }
                match arquivo(fd as u64) {
                    // POLLERR, POLLHUP e POLLNVAL sempre valem, mesmo sem pedir
                    Ok(arq) => arq.prontidao() & (eventos | POLLERR | POLLHUP),
                    Err(_) => POLLNVAL,
                }
            })
            .collect();
        let agora = pit::milissegundos();
        if respostas.iter().any(|&r| r != 0) || limite.is_some_and(|l| agora >= l) {
            return Ok(respostas);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::atual().prazo_ms = limite.unwrap_or(0);
        processo::dormir(processo::CANAL_POLL);
        processo::atual().prazo_ms = 0;
    }
}

/// `poll(fds, n, timeout_ms)`: cada `struct pollfd` tem 8 bytes: fd, eventos, resposta.
fn sys_poll(ptr: u64, quantos: u64, prazo_ms: Option<u64>) -> Resultado {
    if quantos > 1024 {
        return Err(EINVAL);
    }
    let bruto = ler_usuario(ptr, quantos as usize * 8)?;
    let pedidos: Vec<(i32, u16)> = bruto
        .chunks_exact(8)
        .map(|c| (i32::from_le_bytes(c[..4].try_into().unwrap()), u16::from_le_bytes([c[4], c[5]])))
        .collect();
    let respostas = esperar_descritores(&pedidos, prazo_ms)?;
    let mut saida = bruto;
    for (i, r) in respostas.iter().enumerate() {
        saida[i * 8 + 6..i * 8 + 8].copy_from_slice(&r.to_le_bytes());
    }
    escrever_usuario(ptr, &saida)?;
    Ok(respostas.iter().filter(|&&r| r != 0).count() as u64)
}

/// Prazo em milissegundos a partir de um `struct timespec` (ponteiro nulo = sem prazo).
fn prazo_de_timespec(ptr: u64) -> Result<Option<u64>, i64> {
    if ptr == 0 {
        return Ok(None);
    }
    let b = ler_usuario(ptr, 16)?;
    let s = u64::from_le_bytes(b[..8].try_into().unwrap());
    let ns = u64::from_le_bytes(b[8..].try_into().unwrap());
    Ok(Some(s * 1000 + ns / 1_000_000))
}

fn prazo_de_timeval(ptr: u64) -> Result<Option<u64>, i64> {
    if ptr == 0 {
        return Ok(None);
    }
    let b = ler_usuario(ptr, 16)?;
    let s = u64::from_le_bytes(b[..8].try_into().unwrap());
    let us = u64::from_le_bytes(b[8..].try_into().unwrap());
    Ok(Some(s * 1000 + us / 1000))
}

/// `select(n, leitura, escrita, exceção, prazo)`: conjuntos de bits de descritores.
fn sys_select(n: u64, leitura: u64, escrita: u64, excecao: u64, prazo_ms: Option<u64>) -> Resultado {
    let n = (n as usize).min(MAX_ARQUIVOS);
    let ler_conjunto = |ptr: u64| -> Result<u64, i64> {
        if ptr == 0 { Ok(0) } else { Ok(u64::from_le_bytes(ler_usuario(ptr, 8)?.try_into().unwrap())) }
    };
    let (l, e, x) = (ler_conjunto(leitura)?, ler_conjunto(escrita)?, ler_conjunto(excecao)?);
    let mut pedidos = Vec::new();
    for fd in 0..n {
        let bit = 1u64 << fd;
        let mut eventos = 0;
        if l & bit != 0 {
            eventos |= POLLIN;
        }
        if e & bit != 0 {
            eventos |= POLLOUT;
        }
        if (l | e | x) & bit != 0 {
            pedidos.push((fd as i32, eventos));
        }
    }
    let respostas = esperar_descritores(&pedidos, prazo_ms)?;
    let (mut rl, mut re, mut rx, mut total) = (0u64, 0u64, 0u64, 0);
    for (&(fd, _), &r) in pedidos.iter().zip(&respostas) {
        let bit = 1u64 << fd;
        if r & POLLNVAL != 0 {
            return Err(EBADF);
        }
        if l & bit != 0 && r & (POLLIN | POLLHUP | POLLERR) != 0 {
            rl |= bit;
            total += 1;
        }
        if e & bit != 0 && r & (POLLOUT | POLLERR) != 0 {
            re |= bit;
            total += 1;
        }
        if x & bit != 0 && r & POLLERR != 0 {
            rx |= bit;
            total += 1;
        }
    }
    for (ptr, valor) in [(leitura, rl), (escrita, re), (excecao, rx)] {
        if ptr != 0 {
            escrever_usuario(ptr, &valor.to_le_bytes())?;
        }
    }
    Ok(total)
}

fn sys_gettimeofday(ptr: u64) -> Resultado {
    if ptr != 0 {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&rtc::agora().to_le_bytes());
        b[8..].copy_from_slice(&(pit::milissegundos() % 1000 * 1000).to_le_bytes());
        escrever_usuario(ptr, &b)?;
    }
    Ok(0)
}

fn sys_time(ptr: u64) -> Resultado {
    let agora = rtc::agora();
    if ptr != 0 {
        escrever_usuario(ptr, &agora.to_le_bytes())?;
    }
    Ok(agora)
}

fn sys_pipe(ptr: u64, flags: u64) -> Resultado {
    let (leitura, escrita) = Pipe::novo();
    if flags & crate::arquivo::O_NONBLOCK != 0 {
        leitura.definir_flags(crate::arquivo::O_NONBLOCK);
        escrita.definir_flags(crate::arquivo::O_NONBLOCK);
    }
    let fd_leitura = novo_descritor(leitura)?;
    let fd_escrita = match novo_descritor(escrita) {
        Ok(fd) => fd,
        Err(e) => {
            processo::atual().arquivos().lista[fd_leitura as usize] = None;
            return Err(e);
        }
    };
    if flags & O_CLOEXEC != 0 {
        processo::atual().arquivos().fecha_no_exec |= (1 << fd_leitura) | (1 << fd_escrita);
    }
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&(fd_leitura as i32).to_le_bytes());
    bytes[4..].copy_from_slice(&(fd_escrita as i32).to_le_bytes());
    escrever_usuario(ptr, &bytes)?;
    Ok(0)
}

const S_IFCHR: u32 = 0o020000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFIFO: u32 = 0o010000;
const S_IFSOCK: u32 = 0o140000;
const S_IFLNK: u32 = 0o120000;

/// A `struct stat` do Linux x86_64 (144 bytes); preenchemos os campos principais.
fn montar_stat(id: u64, modo: u32, tamanho: u64, modificado: u64, uid: u32, gid: u32) -> [u8; 144] {
    let mut b = [0u8; 144];
    b[0..8].copy_from_slice(&1u64.to_le_bytes()); // st_dev
    b[8..16].copy_from_slice(&id.to_le_bytes()); // st_ino
    b[16..24].copy_from_slice(&1u64.to_le_bytes()); // st_nlink
    b[24..28].copy_from_slice(&modo.to_le_bytes()); // st_mode
    b[28..32].copy_from_slice(&uid.to_le_bytes()); // st_uid
    b[32..36].copy_from_slice(&gid.to_le_bytes()); // st_gid
    b[48..56].copy_from_slice(&tamanho.to_le_bytes()); // st_size
    b[56..64].copy_from_slice(&4096u64.to_le_bytes()); // st_blksize
    b[64..72].copy_from_slice(&tamanho.div_ceil(512).to_le_bytes()); // st_blocks
    for deslocamento in [72, 88, 104] {
        b[deslocamento..deslocamento + 8].copy_from_slice(&modificado.to_le_bytes()); // atime, mtime, ctime
    }
    b
}

fn stat_do_no(no: &Vnode) -> Result<[u8; 144], i64> {
    let tipo = match no.tipo()? {
        vfs::Tipo::Arquivo => S_IFREG,
        vfs::Tipo::Diretorio => S_IFDIR,
        vfs::Tipo::Dispositivo(_) => S_IFCHR,
        vfs::Tipo::Link => S_IFLNK,
        vfs::Tipo::Fifo => S_IFIFO,
        vfs::Tipo::Socket => S_IFSOCK,
    };
    let m = no.meta()?;
    let mut stat = montar_stat(no.id(), tipo | m.permissoes, no.tamanho()?, m.modificado, m.uid, m.gid);
    // st_rdev: qual terminal é (o `tty` descobre o nome do seu terminal por aqui).
    let rdev = match no.tipo()? {
        vfs::Tipo::Dispositivo(vfs::Dispositivo::Console(n)) => tty::dispositivo(n as u32),
        vfs::Tipo::Dispositivo(vfs::Dispositivo::PtyEscravo(id)) => tty::dispositivo(id),
        vfs::Tipo::Dispositivo(vfs::Dispositivo::Terminal) => 5 << 8,
        vfs::Tipo::Dispositivo(vfs::Dispositivo::Ptmx) => 5 << 8 | 2,
        _ => 0,
    };
    stat[40..48].copy_from_slice(&rdev.to_le_bytes());
    Ok(stat)
}

fn sys_stat(ptr: u64, buf: u64) -> Resultado {
    stat_caminho(&caminho(ptr)?, buf, true)
}

/// `seguir`: stat (do alvo) ou lstat (do próprio link).
fn stat_caminho(caminho: &str, buf: u64, seguir: bool) -> Resultado {
    let no = if seguir { vfs::buscar_como(caminho, cred())? } else { vfs::buscar_como_sem_seguir(caminho, cred())? };
    escrever_usuario(buf, &stat_do_no(&no)?)?;
    Ok(0)
}

/// `symlink(alvo, caminho)`: o alvo é guardado como texto, sem ser conferido
/// (um link pode apontar para algo que ainda não existe).
fn sys_symlink(ptr_alvo: u64, caminho: Result<String, i64>) -> Resultado {
    let caminho = caminho?;
    let alvo = espaco().ler_string(ptr_alvo, 4096).ok_or(EFAULT)?;
    if alvo.is_empty() {
        return Err(ENOENT);
    }
    pai_gravavel(&caminho)?;
    let c = cred();
    let meta = vfs::Meta { permissoes: 0o777, uid: c.uid, gid: c.gid, modificado: rtc::agora() };
    vfs::criar_link_simbolico(&caminho, &alvo, meta).map(|_| 0)
}

/// `readlink`: copia o alvo (sem o zero do fim, como no Linux) e devolve o tamanho.
fn sys_readlink(caminho: Result<String, i64>, buf: u64, tamanho: u64) -> Resultado {
    let alvo = vfs::buscar_como_sem_seguir(&caminho?, cred())?.ler_link()?;
    let n = alvo.len().min(tamanho as usize);
    escrever_usuario(buf, &alvo.as_bytes()[..n])?;
    Ok(n as u64)
}

/// `link(existente, novo)`: mais um nome para o mesmo arquivo.
fn sys_link(existente: Result<String, i64>, novo: Result<String, i64>) -> Resultado {
    let (existente, novo) = (existente?, novo?);
    vfs::buscar_como_sem_seguir(&existente, cred())?;
    pai_gravavel(&novo)?;
    vfs::criar_link(&existente, &novo).map(|_| 0)
}

fn sys_fstat(fd: u64, buf: u64) -> Resultado {
    let arq = arquivo(fd)?;
    let stat = match (arq.vnode(), &arq.tipo) {
        (Some(no), _) => stat_do_no(no)?,
        (None, Tipo::Socket(_)) => montar_stat(0, S_IFSOCK | 0o777, 0, 0, 0, 0),
        (None, Tipo::Epoll(_)) => montar_stat(0, 0o600, 0, 0, 0, 0),
        (None, _) => montar_stat(0, S_IFIFO | 0o600, 0, 0, 0, 0),
    };
    escrever_usuario(buf, &stat)?;
    Ok(0)
}

/// Lista um diretório no formato `linux_dirent64`:
/// inode (8) | deslocamento (8) | tamanho do registro (2) | tipo (1) | nome\0
fn sys_getdents64(fd: u64, buf: u64, tamanho: u64) -> Resultado {
    let arq = arquivo(fd)?;
    let Tipo::No(no) = &arq.tipo else { return Err(ENOTDIR) };
    let mut entradas: Vec<(String, u64, u8)> = vec![(".".into(), no.id(), 4), ("..".into(), no.id(), 4)];
    for (nome, id, tipo) in no.listar()? {
        let tipo = match tipo {
            vfs::Tipo::Diretorio => 4,       // DT_DIR
            vfs::Tipo::Arquivo => 8,         // DT_REG
            vfs::Tipo::Dispositivo(_) => 2,  // DT_CHR
            vfs::Tipo::Link => 10,           // DT_LNK
            vfs::Tipo::Fifo => 1,            // DT_FIFO
            vfs::Tipo::Socket => 12,         // DT_SOCK
        };
        entradas.push((nome, id, tipo));
    }

    let mut posicao = arq.posicao.lock();
    let mut saida = Vec::new();
    for (i, (nome, id, tipo)) in entradas.iter().enumerate().skip(*posicao as usize) {
        let registro = (19 + nome.len() + 1).next_multiple_of(8);
        if saida.len() + registro > tamanho as usize {
            if saida.is_empty() {
                return Err(EINVAL);
            }
            break;
        }
        saida.extend(id.to_le_bytes());
        saida.extend(((i + 1) as u64).to_le_bytes());
        saida.extend((registro as u16).to_le_bytes());
        saida.push(*tipo);
        saida.extend(nome.as_bytes());
        saida.resize(saida.len() + registro - 19 - nome.len(), 0);
        *posicao = i as u64 + 1;
    }
    drop(posicao);
    escrever_usuario(buf, &saida)?;
    Ok(saida.len() as u64)
}

fn sys_getcwd(buf: u64, tamanho: u64) -> Resultado {
    let mut bytes = processo::atual().fs().diretorio.clone().into_bytes();
    bytes.push(0);
    if bytes.len() as u64 > tamanho {
        return Err(ERANGE);
    }
    escrever_usuario(buf, &bytes)?;
    Ok(bytes.len() as u64)
}

fn sys_chdir(ptr: u64) -> Resultado {
    let caminho = caminho(ptr)?;
    let no = vfs::buscar_como(&caminho, cred())?;
    if no.tipo()? != vfs::Tipo::Diretorio {
        return Err(ENOTDIR);
    }
    no.permite(cred(), vfs::EXECUTAR)?;
    processo::atual().fs().diretorio = caminho;
    Ok(0)
}

fn sys_rename(de: u64, para: u64) -> Resultado {
    let (de, para) = (caminho(de)?, caminho(para)?);
    pode_apagar(&de)?;
    pai_gravavel(&para)?;
    vfs::renomear(&de, &para).map(|_| 0)
}

// -----------------------------------------------------------------------------
// Processos e memória
// -----------------------------------------------------------------------------

/// Lê um vetor de strings terminado em NULL (argv, envp).
fn ler_vetor(ptr: u64, maximo: u64) -> Result<Vec<String>, i64> {
    let mut strings = Vec::new();
    if ptr == 0 {
        return Ok(strings);
    }
    for i in 0.. {
        if i == maximo {
            return Err(EINVAL);
        }
        let item = u64::from_le_bytes(ler_usuario(ptr + i * 8, 8)?.try_into().unwrap());
        if item == 0 {
            break;
        }
        strings.push(espaco().ler_string(item, 4096).ok_or(EFAULT)?);
    }
    Ok(strings)
}

fn sys_execve(ctx: &mut Contexto, ptr_caminho: u64, ptr_args: u64, ptr_ambiente: u64) -> Resultado {
    let caminho = caminho(ptr_caminho)?;
    let args = ler_vetor(ptr_args, 64)?;
    let ambiente = ler_vetor(ptr_ambiente, 128)?;
    processo::exec(ctx, &caminho, args, ambiente)?;
    Ok(0)
}

/// `futex(endereço, operação, valor, prazo ou valor2, endereço2, valor3)`.
fn sys_futex(endereco: u64, operacao: u64, valor: u64, prazo: u64, destino: u64, valor3: u64) -> Resultado {
    const WAIT: u64 = 0;
    const WAKE: u64 = 1;
    const REQUEUE: u64 = 3;
    const CMP_REQUEUE: u64 = 4;
    const WAIT_BITSET: u64 = 9;
    const WAKE_BITSET: u64 = 10;
    match operacao & 0x7F {
        // FUTEX_PRIVATE_FLAG (128) e FUTEX_CLOCK_REALTIME (256) não mudam nada aqui
        WAIT => processo::futex_esperar(endereco, valor as u32, prazo_de_timespec(prazo)?),
        WAIT_BITSET => {
            // Aqui o prazo é absoluto (no relógio monotônico).
            let prazo = prazo_de_timespec(prazo)?.map(|absoluto| absoluto.saturating_sub(pit::milissegundos()));
            processo::futex_esperar(endereco, valor as u32, prazo)
        }
        WAKE | WAKE_BITSET => processo::futex_acordar(endereco, valor),
        REQUEUE => processo::futex_mudar_fila(endereco, valor, prazo, destino, None),
        CMP_REQUEUE => processo::futex_mudar_fila(endereco, valor, prazo, destino, Some(valor3 as u32)),
        _ => Err(ENOSYS),
    }
}

/// `clone` sem threads: um `fork` em que o filho começa noutra pilha (é
/// assim que a libc implementa `posix_spawn`, `system` e `popen`). Com
/// CLONE_VM a libc espera memória compartilhada, mas o único uso disso no
/// caminho do spawn é a pilha do filho — que uma cópia (copy-on-write) atende.
fn sys_clone(ctx: &mut Contexto, flags: u64, pilha: u64, ptid: u64, ctid: u64, tls: u64) -> Resultado {
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_THREAD: u64 = 0x0001_0000;
    if flags & CLONE_THREAD != 0 {
        return processo::criar_thread(ctx, flags, pilha, ptid, ctid, tls).map(|t| t as u64);
    }
    let mut ctx_filho = *ctx;
    if pilha != 0 {
        ctx_filho.rsp = pilha;
    }
    let pid = processo::fork(&ctx_filho)?;
    if flags & CLONE_SETTLS != 0 {
        processo::definir_fs_de(pid, tls);
    }
    Ok(pid as u64)
}

const FS_BASE: u32 = 0xC000_0100;

/// `arch_prctl`: aqui, só para o registrador FS, que a libc aponta para a
/// área da thread (TLS) — é lá que mora, por exemplo, o `errno`.
fn sys_arch_prctl(codigo: u64, endereco: u64) -> Resultado {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    let p = processo::atual();
    match codigo {
        ARCH_SET_FS => {
            p.fs_base = endereco;
            escrever_msr(FS_BASE, endereco);
            Ok(0)
        }
        ARCH_GET_FS => {
            escrever_usuario(endereco, &p.fs_base.to_le_bytes())?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

pub fn definir_fs(valor: u64) {
    escrever_msr(FS_BASE, valor);
}

fn sys_wait4(pid: u64, ptr_status: u64, opcoes: u64) -> Resultado {
    let (pid, status) = processo::esperar(pid as i32 as i64, opcoes)?;
    if ptr_status != 0 && pid != 0 {
        escrever_usuario(ptr_status, &status.to_le_bytes())?;
    }
    Ok(pid as u64)
}

/// `brk`: move o fim do heap do programa. É o que o `malloc` usa por baixo.
fn sys_brk(novo: u64) -> Resultado {
    let p = processo::atual();
    let m = p.mem().ok_or(EINVAL)?;
    if novo < m.brk_inicio || novo >= m.mmap_topo.saturating_sub(0x1000_0000) {
        return Ok(m.brk);
    }
    // O heap é uma região como as outras: crescer não aloca nada (as páginas
    // vêm sob demanda); encolher devolve as páginas que sobraram.
    m.espaco.redimensionar_regiao(m.brk_inicio, m.brk, novo);
    m.brk = novo;
    Ok(novo)
}

/// `rt_sigaction`: a `struct sigaction` do kernel tem 4 campos de 8 bytes:
/// tratador, flags, restaurador e máscara.
fn sys_sigaction(numero: u64, nova: u64, antiga: u64) -> Resultado {
    if numero == 0 || numero >= 64 || sinal::INBLOQUEAVEIS & (1 << numero) != 0 {
        return Err(EINVAL);
    }
    let p = processo::atual();
    if antiga != 0 {
        let a = p.sinais.acoes()[numero as usize];
        let mut b = Vec::new();
        for valor in [a.tratador, a.flags, a.restaurador, a.mascara] {
            b.extend(valor.to_le_bytes());
        }
        escrever_usuario(antiga, &b)?;
    }
    if nova != 0 {
        let b = ler_usuario(nova, 32)?;
        let campo = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        let acao = sinal::Acao { tratador: campo(0), flags: campo(1), restaurador: campo(2), mascara: campo(3) };
        p.sinais.acoes()[numero as usize] = acao;
        if acao.tratador == sinal::SIG_IGN || (acao.tratador == sinal::SIG_DFL && sinal::padrao_e_ignorar(numero as u32)) {
            p.sinais.pendentes &= !(1 << numero); // POSIX: ignorar descarta o pendente
        }
    }
    Ok(0)
}

/// `rt_sigprocmask`: 0 = bloquear mais, 1 = desbloquear, 2 = trocar a máscara.
fn sys_sigprocmask(como: u64, nova: u64, antiga: u64) -> Resultado {
    let p = processo::atual();
    if antiga != 0 {
        escrever_usuario(antiga, &p.sinais.bloqueados.to_le_bytes())?;
    }
    if nova != 0 {
        let mascara = u64::from_le_bytes(ler_usuario(nova, 8)?.try_into().unwrap());
        let s = &mut p.sinais;
        s.bloqueados = match como {
            0 => s.bloqueados | mascara,
            1 => s.bloqueados & !mascara,
            2 => mascara,
            _ => return Err(EINVAL),
        } & !sinal::INBLOQUEAVEIS;
    }
    Ok(0)
}

/// `pause`: dorme até chegar um sinal.
fn sys_pause() -> Resultado {
    loop {
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(processo::CANAL_PAUSA);
    }
}

/// `alarm`: SIGALRM daqui a N segundos (0 cancela). Devolve o que faltava do anterior.
fn sys_alarm(segundos: u64) -> Resultado {
    let p = processo::atual();
    let agora = pit::milissegundos();
    let restante = if p.alarme_ms > agora { (p.alarme_ms - agora).div_ceil(1000) } else { 0 };
    p.alarme_ms = if segundos == 0 { 0 } else { agora + segundos * 1000 };
    Ok(restante)
}

fn sys_nanosleep(ptr: u64) -> Resultado {
    let tempo = ler_usuario(ptr, 16)?;
    let segundos = u64::from_le_bytes(tempo[..8].try_into().unwrap());
    let nanos = u64::from_le_bytes(tempo[8..].try_into().unwrap());
    let alvo = pit::milissegundos() + segundos * 1000 + nanos / 1_000_000;
    while pit::milissegundos() < alvo {
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(processo::CANAL_TEMPO);
    }
    Ok(0)
}

/// Relógios: 0 e 5 (REALTIME): data e hora; 2 e 3 (CPUTIME): tempo de CPU
/// gasto pelo processo (o `clock()` do C); os outros: tempo desde a partida.
fn sys_clock_gettime(relogio: u64, ptr: u64) -> Resultado {
    let ms = match relogio {
        2 | 3 => processo::atual().tiques * 1000 / pit::FREQUENCIA as u64,
        _ => pit::milissegundos(),
    };
    let segundos = if matches!(relogio, 0 | 5) { rtc::agora() } else { ms / 1000 };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&segundos.to_le_bytes());
    b[8..].copy_from_slice(&((ms % 1000) * 1_000_000).to_le_bytes());
    escrever_usuario(ptr, &b)?;
    Ok(0)
}

fn sys_uname(ptr: u64) -> Resultado {
    // 6 campos de 65 bytes: sistema, máquina na rede, versão, compilação, arquitetura, domínio
    let campos = ["FerrugemOS", "ferrugem", "0.4.0", "#1 SMP-nao (uma CPU so)", "x86_64", ""];
    let mut b = [0u8; 6 * 65];
    for (i, campo) in campos.iter().enumerate() {
        b[i * 65..i * 65 + campo.len()].copy_from_slice(campo.as_bytes());
    }
    escrever_usuario(ptr, &b)?;
    Ok(0)
}

fn sys_reboot(comando: u64) -> Resultado {
    if processo::atual().euid != 0 {
        return Err(EPERM);
    }
    // Antes de desligar, o que está no cache de disco vai para o disco.
    vfs::sincronizar_tudo();
    match comando {
        0x4321_FEDC => {
            println!("Desligando...");
            unsafe {
                porta::outw(0x604, 0x2000); // QEMU
                porta::outw(0xB004, 0x2000); // Bochs e QEMU antigo
            }
            Err(ENOSYS)
        }
        0x0123_4567 => {
            println!("Reiniciando...");
            unsafe {
                while porta::inb(0x64) & 0x02 != 0 {}
                porta::outb(0x64, 0xFE); // pulso no reset da CPU via controlador 8042
            }
            Err(ENOSYS)
        }
        _ => Err(EINVAL),
    }
}

/// Registro de 64 bytes por processo:
/// pid (4) | pai (4) | estado (4) | uid (4) | memória KiB (8) | tiques (8) | nome (28) | pgid (4)
fn sys_processos(buf: u64, maximo: u64) -> Resultado {
    let lista = processo::listar();
    let mut saida = Vec::new();
    for p in lista.iter().take(maximo as usize) {
        saida.extend(p.pid.to_le_bytes());
        saida.extend(p.pai.to_le_bytes());
        saida.extend(p.estado.to_le_bytes());
        saida.extend(p.uid.to_le_bytes());
        saida.extend(p.memoria_kib.to_le_bytes());
        saida.extend(p.tiques.to_le_bytes());
        let mut nome = [0u8; 28];
        let n = p.nome.len().min(27);
        nome[..n].copy_from_slice(&p.nome.as_bytes()[..n]);
        saida.extend(nome);
        saida.extend(p.pgid.to_le_bytes());
    }
    escrever_usuario(buf, &saida)?;
    Ok(lista.len().min(maximo as usize) as u64)
}

/// total KiB | livre KiB | heap do kernel KiB | heap usado KiB
fn sys_memoria(buf: u64) -> Resultado {
    let (total, livres) = quadros::estatisticas();
    let mut b = Vec::new();
    for valor in [total * 4, livres * 4, heap::TAMANHO / 1024, heap::usado() / 1024] {
        b.extend((valor as u64).to_le_bytes());
    }
    escrever_usuario(buf, &b)?;
    Ok(0)
}

/// blocos no total | blocos livres | tamanho do bloco (tudo zero se não houver disco)
fn sys_disco(buf: u64) -> Resultado {
    let (total, livres, bloco) = vfs::disco().map_or((0, 0, 0), |d| d.estatisticas());
    let mut b = Vec::new();
    for valor in [total, livres, bloco] {
        b.extend(valor.to_le_bytes());
    }
    escrever_usuario(buf, &b)?;
    Ok(0)
}

// -----------------------------------------------------------------------------
// Montagens e sincronização
// -----------------------------------------------------------------------------

fn texto_do_usuario(ptr: u64) -> Result<String, i64> {
    if ptr == 0 {
        return Ok(String::new());
    }
    espaco().ler_string(ptr, 256).ok_or(EFAULT)
}

/// `mount(fonte, alvo, tipo, flags, dados)`: só o root. Tipos: ext2 (de um
/// /dev/hdX), proc e tmpfs.
fn sys_mount(fonte: u64, alvo: u64, tipo: u64) -> Resultado {
    if processo::atual().euid != 0 {
        return Err(EPERM);
    }
    let (fonte, alvo, tipo) = (texto_do_usuario(fonte)?, caminho(alvo)?, texto_do_usuario(tipo)?);
    let raiz = match tipo.as_str() {
        "ext2" => {
            let nome = fonte.strip_prefix("/dev/").ok_or(ENODEV)?;
            let bloco = crate::blocos::buscar(nome).ok_or(ENODEV)?;
            let fs = crate::ext2::Ext2::montar(bloco).map_err(|_| EINVAL)?;
            Vnode::Disco(fs, crate::ext2::RAIZ)
        }
        "proc" => Vnode::Proc(crate::procfs::NoProc::Raiz),
        "tmpfs" => vfs::novo_tmpfs(),
        _ => return Err(ENODEV),
    };
    vfs::montar(&alvo, raiz, &fonte, &tipo).map(|_| 0)
}

fn sys_umount(alvo: u64) -> Resultado {
    if processo::atual().euid != 0 {
        return Err(EPERM);
    }
    vfs::desmontar(&caminho(alvo)?).map(|_| 0)
}

fn sys_fsync(fd: u64) -> Resultado {
    let arq = arquivo(fd)?;
    if let Tipo::No(Vnode::Disco(fs, _)) = &arq.tipo {
        fs.sincronizar()?;
    }
    Ok(0)
}

/// `statfs`: a `struct statfs` do Linux (120 bytes): tipo, tamanho do bloco,
/// blocos, livres, disponíveis, inodes, inodes livres...
fn sys_statfs(no: &Vnode, buf: u64) -> Resultado {
    let (tipo, bloco, blocos, livres, inodes, inodes_livres) = vfs::info_do_sistema(no);
    let mut b = [0u8; 120];
    for (i, v) in [tipo, bloco, blocos, livres, livres, inodes, inodes_livres, 0, 255, bloco].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    escrever_usuario(buf, &b)?;
    Ok(0)
}
