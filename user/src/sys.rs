//! Chamadas de sistema (numeração do Linux x86_64).

use alloc::string::String;
use alloc::vec::Vec;
use core::arch::{asm, naked_asm};

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SETPGID: u64 = 109;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGID: u64 = 121;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_CLOCK_GETTIME: u64 = 228;

pub type Errno = i32;
pub type Result<T> = core::result::Result<T, Errno>;

pub const ENOENT: Errno = 2;
pub const EINTR: Errno = 4;
pub const EBADF: Errno = 9;
pub const ECHILD: Errno = 10;
pub const EPIPE: Errno = 32;

/// # Safety
/// Os argumentos precisam fazer sentido para a syscall `number`.
pub unsafe fn syscall(number: u64, args: [u64; 6]) -> i64 {
    let result: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as i64 => result,
            in("rdi") args[0],
            in("rsi") args[1],
            in("rdx") args[2],
            in("r10") args[3],
            in("r8") args[4],
            in("r9") args[5],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        )
    }
    result
}

/// Converte o retorno cru (-errno em caso de erro) num `Result`.
fn check(result: i64) -> Result<u64> {
    if (-4095..0).contains(&result) { Err(-result as Errno) } else { Ok(result as u64) }
}

fn call(number: u64, args: [u64; 6]) -> Result<u64> {
    check(unsafe { syscall(number, args) })
}

/// Uma string terminada em zero, para passar caminhos ao kernel.
fn cstring(s: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len() + 1);
    bytes.extend_from_slice(s.as_bytes());
    bytes.push(0);
    bytes
}

/// Mensagem (em português) para um código de erro.
pub fn strerror(errno: Errno) -> &'static str {
    match errno {
        1 => "operação não permitida",
        2 => "arquivo ou diretório não encontrado",
        3 => "processo não encontrado",
        4 => "chamada interrompida",
        5 => "erro de entrada/saída",
        7 => "lista de argumentos longa demais",
        8 => "formato de executável inválido",
        9 => "descritor de arquivo inválido",
        10 => "nenhum processo filho",
        11 => "recurso temporariamente indisponível",
        12 => "memória insuficiente",
        13 => "permissão negada",
        14 => "endereço inválido",
        17 => "arquivo já existe",
        20 => "não é um diretório",
        21 => "é um diretório",
        22 => "argumento inválido",
        24 => "arquivos abertos demais",
        25 => "não é um terminal",
        29 => "busca inválida",
        32 => "pipe quebrado",
        38 => "função não implementada",
        39 => "diretório não está vazio",
        _ => "erro desconhecido",
    }
}

// --- E/S básica -----------------------------------------------------------

pub fn write(fd: u64, data: &[u8]) -> i64 {
    unsafe { syscall(SYS_WRITE, [fd, data.as_ptr() as u64, data.len() as u64, 0, 0, 0]) }
}

pub fn read(fd: u64, buf: &mut [u8]) -> i64 {
    unsafe { syscall(SYS_READ, [fd, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0]) }
}

/// Escreve tudo, repetindo em caso de escrita parcial.
pub fn write_all(fd: u64, mut data: &[u8]) -> Result<()> {
    while !data.is_empty() {
        let n = check(write(fd, data))?;
        data = &data[n as usize..];
    }
    Ok(())
}

// --- Arquivos -------------------------------------------------------------

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;
pub const O_DIRECTORY: u32 = 0x1_0000;
pub const O_CLOEXEC: u32 = 0x8_0000;

pub fn open(path: &str, flags: u32, mode: u32) -> Result<u64> {
    let path = cstring(path);
    call(SYS_OPEN, [path.as_ptr() as u64, flags as u64, mode as u64, 0, 0, 0])
}

pub fn close(fd: u64) -> Result<()> {
    call(SYS_CLOSE, [fd, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn lseek(fd: u64, offset: i64, whence: u32) -> Result<u64> {
    call(SYS_LSEEK, [fd, offset as u64, whence as u64, 0, 0, 0])
}

pub fn dup(fd: u64) -> Result<u64> {
    call(SYS_DUP, [fd, 0, 0, 0, 0, 0])
}

pub fn dup2(old: u64, new: u64) -> Result<u64> {
    call(SYS_DUP2, [old, new, 0, 0, 0, 0])
}

pub fn fcntl(fd: u64, command: u64, arg: u64) -> Result<u64> {
    call(SYS_FCNTL, [fd, command, arg, 0, 0, 0])
}

/// (ponta de leitura, ponta de escrita).
pub fn pipe() -> Result<(u64, u64)> {
    let mut fds = [0i32; 2];
    call(SYS_PIPE, [fds.as_mut_ptr() as u64, 0, 0, 0, 0, 0])?;
    Ok((fds[0] as u64, fds[1] as u64))
}

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;

pub struct Stat {
    pub ino: u64,
    pub mode: u32,
    pub size: u64,
}

impl Stat {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
}

fn parse_stat(raw: &[u8; 144]) -> Stat {
    Stat {
        ino: u64::from_le_bytes(raw[8..16].try_into().unwrap()),
        mode: u32::from_le_bytes(raw[24..28].try_into().unwrap()),
        size: u64::from_le_bytes(raw[48..56].try_into().unwrap()),
    }
}

pub fn stat(path: &str) -> Result<Stat> {
    let path = cstring(path);
    let mut raw = [0u8; 144];
    call(SYS_STAT, [path.as_ptr() as u64, raw.as_mut_ptr() as u64, 0, 0, 0, 0])?;
    Ok(parse_stat(&raw))
}

pub fn fstat(fd: u64) -> Result<Stat> {
    let mut raw = [0u8; 144];
    call(SYS_FSTAT, [fd, raw.as_mut_ptr() as u64, 0, 0, 0, 0])?;
    Ok(parse_stat(&raw))
}

/// Nomes das entradas de um diretório aberto (inclui "." e "..").
pub fn read_dir(fd: u64) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = call(SYS_GETDENTS64, [fd, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0])? as usize;
        if n == 0 {
            return Ok(names);
        }
        let mut offset = 0;
        while offset < n {
            let record = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
            let name = &buf[offset + 19..offset + record];
            let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            names.push(String::from_utf8_lossy(&name[..len]).into_owned());
            offset += record;
        }
    }
}

pub fn mkdir(path: &str) -> Result<()> {
    let path = cstring(path);
    call(SYS_MKDIR, [path.as_ptr() as u64, 0o755, 0, 0, 0, 0]).map(|_| ())
}

pub fn rmdir(path: &str) -> Result<()> {
    let path = cstring(path);
    call(SYS_RMDIR, [path.as_ptr() as u64, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn unlink(path: &str) -> Result<()> {
    let path = cstring(path);
    call(SYS_UNLINK, [path.as_ptr() as u64, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn chdir(path: &str) -> Result<()> {
    let path = cstring(path);
    call(SYS_CHDIR, [path.as_ptr() as u64, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn getcwd() -> Result<String> {
    let mut buf = [0u8; 512];
    let n = call(SYS_GETCWD, [buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0])? as usize;
    let len = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
    Ok(String::from_utf8_lossy(&buf[..len]).into_owned())
}

// --- Processos ------------------------------------------------------------

pub const WNOHANG: u64 = 1;
pub const WUNTRACED: u64 = 2;

pub fn exit(code: i32) -> ! {
    unsafe { syscall(SYS_EXIT, [code as u64, 0, 0, 0, 0, 0]) };
    unreachable!()
}

pub fn getpid() -> u64 {
    unsafe { syscall(SYS_GETPID, [0; 6]) as u64 }
}

pub fn getppid() -> u64 {
    unsafe { syscall(SYS_GETPPID, [0; 6]) as u64 }
}

pub fn getpgid(pid: u64) -> Result<u64> {
    call(SYS_GETPGID, [pid, 0, 0, 0, 0, 0])
}

pub fn setpgid(pid: u64, pgid: u64) -> Result<()> {
    call(SYS_SETPGID, [pid, pgid, 0, 0, 0, 0]).map(|_| ())
}

/// Devolve 0 no filho e o pid do filho no pai.
pub fn fork() -> Result<u64> {
    call(SYS_FORK, [0; 6])
}

/// Troca o programa do processo. Só volta em caso de erro.
pub fn execve(path: &str, args: &[&str], env: &[&str]) -> Errno {
    let path = cstring(path);
    let args: Vec<Vec<u8>> = args.iter().map(|a| cstring(a)).collect();
    let env: Vec<Vec<u8>> = env.iter().map(|e| cstring(e)).collect();
    let mut argv: Vec<*const u8> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(core::ptr::null());
    let mut envp: Vec<*const u8> = env.iter().map(|e| e.as_ptr()).collect();
    envp.push(core::ptr::null());
    let result =
        unsafe { syscall(SYS_EXECVE, [path.as_ptr() as u64, argv.as_ptr() as u64, envp.as_ptr() as u64, 0, 0, 0]) };
    -result as Errno
}

/// Espera um filho: `pid` > 0, -1 (qualquer) ou -grupo. Devolve
/// (pid, estado); pid 0 com WNOHANG se nenhum mudou.
pub fn waitpid(pid: i64, options: u64) -> Result<(u64, u32)> {
    let mut status = 0u32;
    let pid = call(SYS_WAIT4, [pid as u64, &mut status as *mut u32 as u64, options, 0, 0, 0])?;
    Ok((pid, status))
}

/// Código de saída, se o processo terminou normalmente.
pub fn exit_code(status: u32) -> Option<i32> {
    (status & 0x7F == 0).then_some((status >> 8 & 0xFF) as i32)
}

/// Sinal que encerrou o processo.
pub fn term_signal(status: u32) -> Option<u32> {
    let signal = status & 0x7F;
    (signal != 0 && signal != 0x7F).then_some(signal)
}

/// Sinal que parou o processo.
pub fn stop_signal(status: u32) -> Option<u32> {
    (status & 0xFF == 0x7F).then_some(status >> 8 & 0xFF)
}

pub fn kill(pid: i64, signal: u32) -> Result<()> {
    call(SYS_KILL, [pid as u64, signal as u64, 0, 0, 0, 0]).map(|_| ())
}

// --- Sinais ---------------------------------------------------------------

pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGKILL: u32 = 9;
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGPIPE: u32 = 13;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;

pub const SIGNAL_NAMES: [&str; 32] = [
    "", "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV", "USR2", "PIPE", "ALRM",
    "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU", "URG", "XCPU", "XFSZ", "VTALRM", "PROF",
    "WINCH", "IO", "PWR", "SYS",
];

/// Sinal pelo número ou nome ("9", "KILL", "SIGKILL").
pub fn parse_signal(text: &str) -> Option<u32> {
    if let Ok(number) = text.parse::<u32>() {
        return (number <= 64).then_some(number);
    }
    let name = text.strip_prefix("SIG").unwrap_or(text);
    SIGNAL_NAMES.iter().position(|n| !n.is_empty() && *n == name).map(|i| i as u32)
}

pub fn signal_description(signal: u32) -> &'static str {
    match signal {
        1 => "Desconectado",
        2 => "Interrompido",
        3 => "Saída forçada",
        4 => "Instrução ilegal",
        6 => "Abortado",
        8 => "Exceção de ponto flutuante",
        9 => "Morto",
        11 => "Falha de segmentação",
        13 => "Pipe quebrado",
        15 => "Terminado",
        19 | 20 | 21 | 22 => "Parado",
        _ => "Sinal desconhecido",
    }
}

pub enum Handler {
    Default,
    Ignore,
    Function(extern "C" fn(i32)),
}

const SA_RESTORER: u64 = 0x0400_0000;
pub const SA_RESTART: u64 = 0x1000_0000;

/// Chamado ao fim de um tratador (o `ret` dele cai aqui).
#[unsafe(naked)]
unsafe extern "C" fn restore_rt() -> ! {
    naked_asm!("mov eax, 15", "syscall", "ud2")
}

/// Instala a ação de um sinal (`flags` pode ter SA_RESTART).
pub fn sigaction(signal: u32, handler: Handler, flags: u64) -> Result<()> {
    let handler = match handler {
        Handler::Default => 0,
        Handler::Ignore => 1,
        Handler::Function(f) => f as usize as u64,
    };
    let action = [handler, flags | SA_RESTORER, restore_rt as *const () as u64, 0];
    call(SYS_RT_SIGACTION, [signal as u64, action.as_ptr() as u64, 0, 8, 0, 0]).map(|_| ())
}

pub const SIG_BLOCK: u64 = 0;
pub const SIG_UNBLOCK: u64 = 1;
pub const SIG_SETMASK: u64 = 2;

/// Altera a máscara de sinais bloqueados e devolve a anterior.
pub fn sigprocmask(how: u64, set: u64) -> Result<u64> {
    let mut old = 0u64;
    call(SYS_RT_SIGPROCMASK, [how, &set as *const u64 as u64, &mut old as *mut u64 as u64, 8, 0, 0])?;
    Ok(old)
}

pub fn signal_bit(signal: u32) -> u64 {
    1 << (signal - 1)
}

// --- Terminal -------------------------------------------------------------

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;

/// O `struct termios` do kernel (36 bytes).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Termios {
    pub iflag: u32,
    pub oflag: u32,
    pub cflag: u32,
    pub lflag: u32,
    pub line: u8,
    pub cc: [u8; 19],
}

pub const ICANON: u32 = 0x2;
pub const ECHO: u32 = 0x8;

pub fn tcgetattr(fd: u64) -> Result<Termios> {
    let mut termios = Termios { iflag: 0, oflag: 0, cflag: 0, lflag: 0, line: 0, cc: [0; 19] };
    call(SYS_IOCTL, [fd, TCGETS, &mut termios as *mut Termios as u64, 0, 0, 0])?;
    Ok(termios)
}

pub fn tcsetattr(fd: u64, termios: &Termios) -> Result<()> {
    call(SYS_IOCTL, [fd, TCSETS, termios as *const Termios as u64, 0, 0, 0]).map(|_| ())
}

pub fn isatty(fd: u64) -> bool {
    tcgetattr(fd).is_ok()
}

pub fn tcgetpgrp(fd: u64) -> Result<u64> {
    let mut pgid = 0u32;
    call(SYS_IOCTL, [fd, TIOCGPGRP, &mut pgid as *mut u32 as u64, 0, 0, 0])?;
    Ok(pgid as u64)
}

pub fn tcsetpgrp(fd: u64, pgid: u64) -> Result<()> {
    let pgid = pgid as u32;
    call(SYS_IOCTL, [fd, TIOCSPGRP, &pgid as *const u32 as u64, 0, 0, 0]).map(|_| ())
}

// --- Memória, tempo e sistema ---------------------------------------------

/// Define o fim do heap; `brk(0)` só consulta.
pub fn brk(end: u64) -> u64 {
    unsafe { syscall(SYS_BRK, [end, 0, 0, 0, 0, 0]) as u64 }
}

/// Reserva `len` bytes de memória anônima, legível e gravável.
pub fn mmap_anonymous(len: usize) -> core::result::Result<*mut u8, i64> {
    const PROT_READ_WRITE: u64 = 3;
    const MAP_PRIVATE_ANONYMOUS: u64 = 0x22;
    let result = unsafe { syscall(SYS_MMAP, [0, len as u64, PROT_READ_WRITE, MAP_PRIVATE_ANONYMOUS, u64::MAX, 0]) };
    if result < 0 { Err(-result) } else { Ok(result as *mut u8) }
}

pub fn munmap(ptr: *mut u8, len: usize) -> i64 {
    unsafe { syscall(SYS_MUNMAP, [ptr as u64, len as u64, 0, 0, 0, 0]) }
}

pub fn sched_yield() {
    unsafe { syscall(SYS_SCHED_YIELD, [0; 6]) };
}

/// Dorme; devolve `Err(EINTR)` se um sinal interromper.
pub fn sleep_ms(ms: u64) -> Result<()> {
    let request = [ms / 1000, (ms % 1000) * 1_000_000];
    call(SYS_NANOSLEEP, [request.as_ptr() as u64, 0, 0, 0, 0, 0]).map(|_| ())
}

/// Milissegundos de um relógio monotônico.
pub fn clock_ms() -> u64 {
    const CLOCK_MONOTONIC: u64 = 1;
    let mut time = [0u64; 2];
    unsafe { syscall(SYS_CLOCK_GETTIME, [CLOCK_MONOTONIC, time.as_mut_ptr() as u64, 0, 0, 0, 0]) };
    time[0] * 1000 + time[1] / 1_000_000
}

pub struct Utsname([[u8; 65]; 6]);

impl Utsname {
    /// 0 = sistema, 1 = nome da máquina, 2 = versão, 3 = build, 4 = arquitetura.
    pub fn field(&self, index: usize) -> &str {
        let field = &self.0[index];
        let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        core::str::from_utf8(&field[..len]).unwrap_or("?")
    }
}

pub fn uname() -> Utsname {
    let mut info = Utsname([[0; 65]; 6]);
    unsafe { syscall(SYS_UNAME, [info.0.as_mut_ptr() as u64, 0, 0, 0, 0, 0]) };
    info
}
