//! Chamadas de sistema: a porta de entrada dos programas para o kernel.
//!
//! O programa põe o número da chamada em RAX e os argumentos em RDI, RSI,
//! RDX, R10, R8 e R9, e executa a instrução `syscall` (ou `int 0x80`). A CPU
//! sobe para o anel 0 e cai em `entrada_syscall` (interrupcoes.asm), que monta
//! o mesmo `Contexto` de uma interrupção. A resposta volta em RAX; um erro é
//! um número negativo (-ENOENT...).
//!
//! Os números e o formato das estruturas são os mesmos do Linux x86_64.

use crate::arquivo::{Arquivo, Pipe, Tipo};
use crate::erro::*;
use crate::interrupcoes::Contexto;
use crate::paginacao::{ESCRITA, EspacoEnderecos, USUARIO};
use crate::processo::{self, MAX_ARQUIVOS, PILHA_TAMANHO, PILHA_TOPO};
use crate::vfs::{self, Vnode};
use crate::{heap, pit, porta, println, quadros, rtc, sinal, tty};
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

type Resultado = Result<u64, i64>;

pub fn tratar(ctx: &mut Contexto) {
    let numero = ctx.rax;
    let (a1, a2, a3) = (ctx.rdi, ctx.rsi, ctx.rdx);
    let resultado = match numero {
        0 => sys_read(a1, a2, a3),
        1 => sys_write(a1, a2, a3),
        2 => sys_open(a1, a2, a3),
        6 => sys_stat(a1, a2), // lstat: ainda não há links simbólicos
        9 => sys_mmap(a1, a2, ctx.r10, ctx.r8),
        10 => Ok(0), // mprotect: todas as páginas ficam leitura+escrita (simplificação)
        11 => sys_munmap(a1, a2),
        19 => sys_vetorial(a1, a2, a3, false),
        20 => sys_vetorial(a1, a2, a3, true),
        21 => sys_access(caminho(a1), a2),
        24 => {
            processo::escalonar(); // sched_yield: cede a vez
            Ok(0)
        }
        28 => Ok(0), // madvise: só um conselho; pode ser ignorado
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
        39 => Ok(processo::atual().pid as u64),
        56 => sys_clone(ctx, a1, a2, ctx.r8),
        57 | 58 => processo::fork(ctx).map(|pid| pid as u64), // vfork vira fork
        59 => sys_execve(ctx, a1, a2, a3),
        60 | 231 => processo::sair(((a1 & 0xFF) << 8) as i32),
        61 => sys_wait4(a1, a2, a3),
        62 => processo::pode_sinalizar(a1 as u32).and_then(|_| processo::enviar_sinal(a1 as u32, a2 as u32)).map(|_| 0),
        63 => sys_uname(a1),
        72 => sys_fcntl(a1, a2, a3),
        74 | 75 => arquivo(a1).map(|_| 0), // fsync/fdatasync: o ext2 já grava direto
        77 => sys_ftruncate(a1, a2),
        89 => Err(EINVAL), // readlink: não há links simbólicos
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
        110 => Ok(processo::atual().pai as u64),
        158 => sys_arch_prctl(a1, a2),
        200 => processo::enviar_sinal(a1 as u32, a2 as u32).map(|_| 0), // tkill: thread = processo
        201 => sys_time(a1),
        218 => Ok(processo::atual().pid as u64), // set_tid_address: uma thread por processo
        230 => sys_nanosleep(a3), // clock_nanosleep (sem TIMER_ABSTIME)
        257 => caminho_at(a1, a2).and_then(|c| abrir_caminho(c, a3, ctx.r10)),
        258 => caminho_at(a1, a2).and_then(|c| mkdir_caminho(c, a3)),
        234 => processo::pode_sinalizar(a2 as u32).and_then(|_| processo::enviar_sinal(a2 as u32, a3 as u32)).map(|_| 0), // tgkill
        262 => sys_newfstatat(a1, a2, a3, ctx.r10),
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
    if resultado == Err(EINTR) && processo::reiniciar_syscall() {
        ctx.rax = numero;
        ctx.rip -= 2;
        return;
    }
    ctx.rax = match resultado {
        Ok(valor) => valor,
        Err(erro) => (-erro) as u64,
    };
}

// -----------------------------------------------------------------------------
// Acesso à memória do programa
// -----------------------------------------------------------------------------

fn espaco() -> &'static EspacoEnderecos {
    processo::atual().espaco.as_ref().expect("syscall sem espaço de usuário")
}

fn ler_usuario(ptr: u64, tamanho: usize) -> Result<Vec<u8>, i64> {
    espaco().copiar_de(ptr, tamanho).ok_or(EFAULT)
}

fn escrever_usuario(ptr: u64, dados: &[u8]) -> Result<(), i64> {
    if espaco().copiar_para(ptr, dados) { Ok(()) } else { Err(EFAULT) }
}

/// Lê um caminho do programa e já o resolve em relação ao diretório atual.
fn caminho(ptr: u64) -> Result<String, i64> {
    let texto = espaco().ler_string(ptr, 4096).ok_or(EFAULT)?;
    if texto.is_empty() {
        return Err(ENOENT);
    }
    Ok(vfs::normalizar(&processo::atual().diretorio, &texto))
}

fn arquivo(fd: u64) -> Result<Arc<Arquivo>, i64> {
    processo::atual()
        .arquivos
        .get(fd as usize)
        .and_then(|a| a.clone())
        .ok_or(EBADF)
}

/// Põe o arquivo no menor descritor livre (regra do Unix: por isso `close(1)`
/// seguido de `open` faz o arquivo virar a saída padrão).
fn novo_descritor(arq: Arc<Arquivo>) -> Resultado {
    let tabela = &mut processo::atual().arquivos;
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

fn cred() -> vfs::Cred {
    processo::atual().cred()
}

/// Permissões de um arquivo novo: as pedidas, menos as da umask.
fn permissoes_novas(pedidas: u64) -> u32 {
    pedidas as u32 & 0o7777 & !processo::atual().umask
}

const O_CLOEXEC: u64 = 0o2000000;
const AT_FDCWD: u64 = -100i64 as u64;

fn sys_open(ptr: u64, flags: u64, permissoes: u64) -> Resultado {
    abrir_caminho(caminho(ptr)?, flags, permissoes)
}

fn abrir_caminho(caminho: String, flags: u64, permissoes: u64) -> Resultado {
    let fd = novo_descritor(Arquivo::abrir(&caminho, flags, cred(), permissoes_novas(permissoes))?)?;
    if flags & O_CLOEXEC != 0 {
        processo::atual().fecha_no_exec |= 1 << fd;
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
fn pai_gravavel(caminho: &str) -> Result<Vnode, i64> {
    let (pai, _) = vfs::separar(caminho)?;
    let pai = vfs::buscar_como(pai, cred())?;
    pai.permite(cred(), vfs::ESCREVER | vfs::EXECUTAR)?;
    Ok(pai)
}

/// Pode apagar (ou mover) `caminho`? Precisa poder escrever no diretório e,
/// se ele tiver o sticky bit (o /tmp), ser dono do arquivo ou do diretório.
fn pode_apagar(caminho: &str) -> Result<(), i64> {
    let pai = pai_gravavel(caminho)?;
    let alvo = vfs::buscar(caminho)?;
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
    let p = processo::atual();
    let antiga = p.umask;
    p.umask = nova as u32 & 0o777;
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
    let eh_terminal = match &arq.tipo {
        Tipo::No(no) => no.tipo()? == vfs::Tipo::Dispositivo(vfs::Dispositivo::Terminal),
        _ => false,
    };
    if !eh_terminal {
        return Err(ENOTTY);
    }
    let dados = if pedido & 0xFF00 == 0x5400 && (0x5402..=0x5404).contains(&pedido) {
        Some(ler_usuario(ptr, 36)?)
    } else {
        None
    };
    let resposta = tty::ioctl(pedido, dados.as_deref(), processo::atual().pid)?;
    if !resposta.is_empty() {
        escrever_usuario(ptr, &resposta)?;
    }
    Ok(0)
}

fn sys_close(fd: u64) -> Resultado {
    let p = processo::atual();
    let entrada = p.arquivos.get_mut(fd as usize).ok_or(EBADF)?;
    entrada.take().ok_or(EBADF)?;
    p.fecha_no_exec &= !(1 << fd);
    Ok(0)
}

/// `dup2`/`dup3`: `novo` passa a apontar para o mesmo arquivo que `antigo`.
fn sys_dup2(antigo: u64, novo: u64, flags: u64) -> Resultado {
    let arq = arquivo(antigo)?;
    let novo = novo as usize;
    if novo >= MAX_ARQUIVOS {
        return Err(EBADF);
    }
    let p = processo::atual();
    if p.arquivos.len() <= novo {
        p.arquivos.resize(novo + 1, None);
    }
    p.arquivos[novo] = Some(arq);
    p.fecha_no_exec &= !(1 << novo);
    if flags & O_CLOEXEC != 0 {
        p.fecha_no_exec |= 1 << novo;
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
    let p = processo::atual();
    match comando {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let livre = (argumento as usize..MAX_ARQUIVOS)
                .find(|&i| p.arquivos.get(i).is_none_or(|a| a.is_none()))
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
        F_GETFL => Ok(arq.flags() & 0o3),
        F_SETFL => Ok(0), // O_NONBLOCK etc.: ignorados
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
    const AT_EMPTY_PATH: u64 = 0x1000;
    let vazio = espaco().ler_string(ptr, 1).is_some_and(|s| s.is_empty());
    if flags & AT_EMPTY_PATH != 0 && vazio {
        return sys_fstat(dirfd, buf);
    }
    let no = vfs::buscar_como(&caminho_at(dirfd, ptr)?, cred())?;
    escrever_usuario(buf, &stat_do_no(&no)?)?;
    Ok(0)
}

fn sys_renameat(dir_de: u64, de: u64, dir_para: u64, para: u64) -> Resultado {
    let (de, para) = (caminho_at(dir_de, de)?, caminho_at(dir_para, para)?);
    pode_apagar(&de)?;
    pai_gravavel(&para)?;
    vfs::renomear(&de, &para).map(|_| 0)
}

/// Só esvaziar (tamanho 0) é suportado; é o caso comum (`O_TRUNC` à mão).
fn sys_ftruncate(fd: u64, tamanho: u64) -> Resultado {
    let arq = arquivo(fd)?;
    let Tipo::No(no) = &arq.tipo else { return Err(EINVAL) };
    match tamanho {
        0 => no.truncar().map(|_| 0),
        t if t == no.tamanho()? => Ok(0),
        _ => Err(EINVAL),
    }
}

/// `mmap` anônimo: memória zerada, nova, só deste processo. Mapear arquivos
/// ainda não é suportado. Devolve o endereço.
fn sys_mmap(endereco: u64, tamanho: u64, flags: u64, fd: u64) -> Resultado {
    const MAP_FIXED: u64 = 0x10;
    const MAP_ANONYMOUS: u64 = 0x20;
    let _ = fd; // só mmap anônimo: o descritor é ignorado
    if flags & MAP_ANONYMOUS == 0 {
        return Err(ENOSYS); // mapear arquivos ainda não é suportado
    }
    if tamanho == 0 || tamanho > 1 << 32 {
        return Err(EINVAL);
    }
    let tamanho = (tamanho + 0xFFF) & !0xFFF;
    let p = processo::atual();
    let inicio = if flags & MAP_FIXED != 0 {
        if endereco & 0xFFF != 0 {
            return Err(EINVAL);
        }
        endereco
    } else {
        let inicio = p.mmap_topo - tamanho;
        if inicio < (p.brk + 0x1000_0000) {
            return Err(ENOMEM); // encostou no heap
        }
        p.mmap_topo = inicio;
        inicio
    };
    let espaco = p.espaco.as_mut().ok_or(EINVAL)?;
    if flags & MAP_FIXED != 0 {
        espaco.desalocar_regiao(inicio, inicio + tamanho); // MAP_FIXED substitui o que havia
    }
    if !espaco.alocar_regiao(inicio, inicio + tamanho, USUARIO | ESCRITA) {
        return Err(ENOMEM);
    }
    Ok(inicio)
}

fn sys_munmap(endereco: u64, tamanho: u64) -> Resultado {
    if endereco & 0xFFF != 0 || endereco < crate::paginacao::USUARIO_INICIO {
        return Err(EINVAL);
    }
    let espaco = processo::atual().espaco.as_mut().ok_or(EINVAL)?;
    espaco.desalocar_regiao(endereco, endereco + ((tamanho + 0xFFF) & !0xFFF));
    Ok(0)
}

/// `getrandom`: não há fonte de entropia de verdade; misturamos o contador de
/// ciclos da CPU (bom o bastante para sementes, ruim para criptografia).
fn sys_getrandom(buf: u64, tamanho: u64) -> Resultado {
    let tamanho = (tamanho as usize).min(4096);
    let mut estado = unsafe { core::arch::x86_64::_rdtsc() } | 1;
    let mut dados = Vec::with_capacity(tamanho);
    while dados.len() < tamanho {
        // xorshift64*
        estado ^= estado >> 12;
        estado ^= estado << 25;
        estado ^= estado >> 27;
        dados.extend(estado.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    dados.truncate(tamanho);
    escrever_usuario(buf, &dados)?;
    Ok(tamanho as u64)
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
    let fd_leitura = novo_descritor(leitura)?;
    let fd_escrita = match novo_descritor(escrita) {
        Ok(fd) => fd,
        Err(e) => {
            processo::atual().arquivos[fd_leitura as usize] = None;
            return Err(e);
        }
    };
    if flags & O_CLOEXEC != 0 {
        processo::atual().fecha_no_exec |= (1 << fd_leitura) | (1 << fd_escrita);
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
    };
    let m = no.meta()?;
    Ok(montar_stat(no.id(), tipo | m.permissoes, no.tamanho()?, m.modificado, m.uid, m.gid))
}

fn sys_stat(ptr: u64, buf: u64) -> Resultado {
    let no = vfs::buscar_como(&caminho(ptr)?, cred())?;
    escrever_usuario(buf, &stat_do_no(&no)?)?;
    Ok(0)
}

fn sys_fstat(fd: u64, buf: u64) -> Resultado {
    let stat = match &arquivo(fd)?.tipo {
        Tipo::No(no) => stat_do_no(no)?,
        _ => montar_stat(0, S_IFIFO | 0o600, 0, 0, 0, 0),
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
    let mut bytes = processo::atual().diretorio.clone().into_bytes();
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
    processo::atual().diretorio = caminho;
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

/// `clone` sem threads: um `fork` em que o filho começa noutra pilha (é
/// assim que a libc implementa `posix_spawn`, `system` e `popen`). Com
/// CLONE_VM a libc espera memória compartilhada, mas o único uso disso no
/// caminho do spawn é a pilha do filho — que uma cópia (copy-on-write) atende.
fn sys_clone(ctx: &mut Contexto, flags: u64, pilha: u64, tls: u64) -> Resultado {
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_THREAD: u64 = 0x0001_0000;
    if flags & CLONE_THREAD != 0 {
        return Err(ENOSYS); // threads de verdade ainda não existem
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
    if novo < p.brk_inicio || novo >= PILHA_TOPO - PILHA_TAMANHO - 0x10_0000 {
        return Ok(p.brk);
    }
    if novo > p.brk {
        let espaco = p.espaco.as_mut().ok_or(EINVAL)?;
        if !espaco.alocar_regiao((p.brk + 0xFFF) & !0xFFF, novo, USUARIO | ESCRITA) {
            return Ok(p.brk);
        }
    }
    // Encolher só ajusta o número: as páginas continuam lá (simplificação).
    p.brk = novo;
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
        let a = p.sinais.acoes[numero as usize];
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
        p.sinais.acoes[numero as usize] = acao;
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

/// Relógio 0 (REALTIME): data e hora. Relógio 1 (MONOTONIC): tempo desde a partida.
fn sys_clock_gettime(relogio: u64, ptr: u64) -> Resultado {
    let ms = pit::milissegundos();
    let segundos = if relogio == 0 { rtc::agora() } else { ms / 1000 };
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
/// pid (4) | pai (4) | estado (4) | uid (4) | memória KiB (8) | tiques (8) | nome (32)
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
        let mut nome = [0u8; 32];
        let n = p.nome.len().min(31);
        nome[..n].copy_from_slice(&p.nome.as_bytes()[..n]);
        saida.extend(nome);
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
