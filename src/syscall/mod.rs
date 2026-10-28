//! Chamadas de sistema via instrução `syscall`, com a numeração e as
//! convenções do Linux x86_64: número em rax, argumentos em rdi, rsi, rdx,
//! r10, r8, r9, resultado em rax (negativo = -errno). Assim, binários
//! estáticos feitos para o Linux rodam aqui sem mudança, enquanto usarem só
//! o que já foi implementado.

mod fs;
mod proc;

use alloc::string::String;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::fmt::Write;

use crate::cpu::{rdmsr, wrmsr};
use crate::interrupts::{self, InterruptFrame};
use crate::paging::USER_TOP;
use crate::process::{self, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::serial::SERIAL;
use crate::task::{self, WaitStatus};
use crate::{gdt, signal, timer};

pub type Errno = i64;
pub const EPERM: Errno = 1;
pub const ENOENT: Errno = 2;
pub const ESRCH: Errno = 3;
pub const EINTR: Errno = 4;
pub const EIO: Errno = 5;
pub const ENXIO: Errno = 6;
pub const E2BIG: Errno = 7;
pub const ENOEXEC: Errno = 8;
pub const EBADF: Errno = 9;
pub const ECHILD: Errno = 10;
pub const EAGAIN: Errno = 11;
pub const ENOMEM: Errno = 12;
pub const EACCES: Errno = 13;
pub const EFAULT: Errno = 14;
pub const EBUSY: Errno = 16;
pub const EEXIST: Errno = 17;
pub const EXDEV: Errno = 18;
pub const ENODEV: Errno = 19;
pub const ENOTDIR: Errno = 20;
pub const EISDIR: Errno = 21;
pub const EINVAL: Errno = 22;
pub const EMFILE: Errno = 24;
pub const ENOTTY: Errno = 25;
pub const EFBIG: Errno = 27;
pub const ENOSPC: Errno = 28;
pub const ESPIPE: Errno = 29;
pub const EROFS: Errno = 30;
pub const EPIPE: Errno = 32;
pub const ERANGE: Errno = 34;
pub const ENAMETOOLONG: Errno = 36;
pub const ENOSYS: Errno = 38;
pub const ENOTEMPTY: Errno = 39;
pub const ELOOP: Errno = 40;

pub type SysResult = Result<i64, Errno>;

const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_FMASK: u32 = 0xC000_0084;

// A instrução `syscall` não troca de pilha: o rsp ainda é o do usuário. Como
// há um só núcleo e as interrupções ficam desligadas na entrada (FMASK),
// basta guardar o rsp do usuário numa variável global e pegar a pilha de
// kernel da tarefa atual. Em seguida montamos o mesmo InterruptFrame de uma
// interrupção vinda do anel 3 e seguimos pelo caminho comum (isr_common), que
// volta ao usuário com `iretq`.
global_asm!(
    r#"
.section .text.syscall, "ax"
.global syscall_entry
syscall_entry:
    movq %rsp, SYSCALL_USER_RSP(%rip)
    movq SYSCALL_KERNEL_RSP(%rip), %rsp
    pushq ${user_data}
    pushq SYSCALL_USER_RSP(%rip)
    pushq %r11                        /* rflags do usuário */
    pushq ${user_code}
    pushq %rcx                        /* rip de retorno */
    pushq $0                          /* código de erro */
    pushq ${mark}                     /* "vetor": isto é uma syscall */
    jmp isr_common
"#,
    user_data = const gdt::USER_DATA as u64,
    user_code = const gdt::USER_CODE as u64,
    mark = const crate::interrupts::SYSCALL_MARK,
    options(att_syntax)
);

unsafe extern "C" {
    fn syscall_entry();
}

#[unsafe(no_mangle)]
static mut SYSCALL_KERNEL_RSP: u64 = 0;
#[unsafe(no_mangle)]
static mut SYSCALL_USER_RSP: u64 = 0;

pub fn init() {
    unsafe {
        wrmsr(MSR_EFER, rdmsr(MSR_EFER) | 1); // SCE: habilita syscall/sysret
        // Seletores de kernel em 47:32; os de usuário (para sysret) em 63:48.
        wrmsr(MSR_STAR, (gdt::KERNEL_DATA as u64) << 48 | (gdt::KERNEL_CODE as u64) << 32);
        wrmsr(MSR_LSTAR, syscall_entry as *const () as u64);
        // Desliga IF, DF, TF e AC ao entrar no kernel.
        wrmsr(MSR_FMASK, 0x4_0700);
    }
}

/// Pilha de kernel da tarefa atual (trocada pelo escalonador).
pub fn set_kernel_stack(top: u64) {
    unsafe { SYSCALL_KERNEL_RSP = top }
}

// Números das syscalls no Linux x86_64.
const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_OPEN: u64 = 2;
const SYS_CLOSE: u64 = 3;
const SYS_STAT: u64 = 4;
const SYS_FSTAT: u64 = 5;
const SYS_LSTAT: u64 = 6;
const SYS_LSEEK: u64 = 8;
const SYS_MMAP: u64 = 9;
const SYS_MUNMAP: u64 = 11;
const SYS_BRK: u64 = 12;
const SYS_RT_SIGACTION: u64 = 13;
const SYS_RT_SIGPROCMASK: u64 = 14;
const SYS_RT_SIGRETURN: u64 = 15;
const SYS_IOCTL: u64 = 16;
const SYS_READV: u64 = 19;
const SYS_WRITEV: u64 = 20;
const SYS_ACCESS: u64 = 21;
const SYS_PIPE: u64 = 22;
const SYS_SCHED_YIELD: u64 = 24;
const SYS_DUP: u64 = 32;
const SYS_DUP2: u64 = 33;
const SYS_NANOSLEEP: u64 = 35;
const SYS_GETPID: u64 = 39;
const SYS_CLONE: u64 = 56;
const SYS_FORK: u64 = 57;
const SYS_VFORK: u64 = 58;
const SYS_EXECVE: u64 = 59;
const SYS_EXIT: u64 = 60;
const SYS_WAIT4: u64 = 61;
const SYS_KILL: u64 = 62;
const SYS_UNAME: u64 = 63;
const SYS_FCNTL: u64 = 72;
const SYS_FSYNC: u64 = 74;
const SYS_FDATASYNC: u64 = 75;
const SYS_TRUNCATE: u64 = 76;
const SYS_FTRUNCATE: u64 = 77;
const SYS_GETCWD: u64 = 79;
const SYS_CHDIR: u64 = 80;
const SYS_MKDIR: u64 = 83;
const SYS_RENAME: u64 = 82;
const SYS_RMDIR: u64 = 84;
const SYS_LINK: u64 = 86;
const SYS_UNLINK: u64 = 87;
const SYS_SYMLINK: u64 = 88;
const SYS_READLINK: u64 = 89;
const SYS_GETUID: u64 = 102;
const SYS_GETGID: u64 = 104;
const SYS_GETEUID: u64 = 107;
const SYS_GETEGID: u64 = 108;
const SYS_SETPGID: u64 = 109;
const SYS_GETPPID: u64 = 110;
const SYS_GETPGRP: u64 = 111;
const SYS_SETSID: u64 = 112;
const SYS_GETPGID: u64 = 121;
const SYS_GETSID: u64 = 124;
const SYS_ARCH_PRCTL: u64 = 158;
const SYS_SYNC: u64 = 162;
const SYS_GETTID: u64 = 186;
const SYS_GETDENTS64: u64 = 217;
const SYS_SET_TID_ADDRESS: u64 = 218;
const SYS_CLOCK_GETTIME: u64 = 228;
const SYS_EXIT_GROUP: u64 = 231;
const SYS_OPENAT: u64 = 257;
const SYS_MKDIRAT: u64 = 258;
const SYS_NEWFSTATAT: u64 = 262;
const SYS_UNLINKAT: u64 = 263;
const SYS_RENAMEAT: u64 = 264;
const SYS_LINKAT: u64 = 265;
const SYS_SYMLINKAT: u64 = 266;
const SYS_READLINKAT: u64 = 267;
const SYS_FACCESSAT: u64 = 269;
const SYS_UTIMENSAT: u64 = 280;
const SYS_DUP3: u64 = 292;
const SYS_PIPE2: u64 = 293;
const SYS_RENAMEAT2: u64 = 316;

pub fn dispatch(frame: &mut InterruptFrame) -> *mut InterruptFrame {
    let number = frame.rax;
    let a = [frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9];
    let result = match number {
        // Arquivos (fs.rs)
        SYS_READ => fs::read(a[0], a[1], a[2]),
        SYS_WRITE => fs::write(a[0], a[1], a[2]),
        SYS_READV => fs::readv(a[0], a[1], a[2]),
        SYS_WRITEV => fs::writev(a[0], a[1], a[2]),
        SYS_OPEN => fs::openat(fs::AT_FDCWD, a[0], a[1], a[2]),
        SYS_OPENAT => fs::openat(a[0] as i64, a[1], a[2], a[3]),
        SYS_CLOSE => fs::close(a[0]),
        SYS_LSEEK => fs::lseek(a[0], a[1], a[2]),
        SYS_STAT => fs::fstatat(fs::AT_FDCWD, a[0], a[1], 0),
        SYS_LSTAT => fs::fstatat(fs::AT_FDCWD, a[0], a[1], fs::AT_SYMLINK_NOFOLLOW),
        SYS_FSTAT => fs::fstat(a[0], a[1]),
        SYS_NEWFSTATAT => fs::fstatat(a[0] as i64, a[1], a[2], a[3]),
        SYS_GETDENTS64 => fs::getdents64(a[0], a[1], a[2]),
        SYS_DUP => fs::dup(a[0]),
        SYS_DUP2 => fs::dup3(a[0], a[1], 0, true),
        SYS_DUP3 => fs::dup3(a[0], a[1], a[2], false),
        SYS_FCNTL => fs::fcntl(a[0], a[1], a[2]),
        SYS_PIPE => fs::pipe2(a[0], 0),
        SYS_PIPE2 => fs::pipe2(a[0], a[1]),
        SYS_IOCTL => fs::ioctl(a[0], a[1], a[2]),
        SYS_GETCWD => fs::getcwd(a[0], a[1]),
        SYS_CHDIR => fs::chdir(a[0]),
        SYS_MKDIR => fs::mkdirat(fs::AT_FDCWD, a[0], a[1]),
        SYS_MKDIRAT => fs::mkdirat(a[0] as i64, a[1], a[2]),
        SYS_RMDIR => fs::unlinkat(fs::AT_FDCWD, a[0], fs::AT_REMOVEDIR),
        SYS_UNLINK => fs::unlinkat(fs::AT_FDCWD, a[0], 0),
        SYS_UNLINKAT => fs::unlinkat(a[0] as i64, a[1], a[2]),
        SYS_ACCESS => fs::faccessat(fs::AT_FDCWD, a[0]),
        SYS_FACCESSAT => fs::faccessat(a[0] as i64, a[1]),
        SYS_RENAME => fs::renameat(fs::AT_FDCWD, a[0], fs::AT_FDCWD, a[1], 0),
        SYS_RENAMEAT => fs::renameat(a[0] as i64, a[1], a[2] as i64, a[3], 0),
        SYS_RENAMEAT2 => fs::renameat(a[0] as i64, a[1], a[2] as i64, a[3], a[4]),
        SYS_LINK => fs::linkat(fs::AT_FDCWD, a[0], fs::AT_FDCWD, a[1]),
        SYS_LINKAT => fs::linkat(a[0] as i64, a[1], a[2] as i64, a[3]),
        SYS_SYMLINK => fs::symlinkat(a[0], fs::AT_FDCWD, a[1]),
        SYS_SYMLINKAT => fs::symlinkat(a[0], a[1] as i64, a[2]),
        SYS_READLINK => fs::readlinkat(fs::AT_FDCWD, a[0], a[1], a[2]),
        SYS_READLINKAT => fs::readlinkat(a[0] as i64, a[1], a[2], a[3]),
        SYS_TRUNCATE => fs::truncate(a[0], a[1]),
        SYS_FTRUNCATE => fs::ftruncate(a[0], a[1]),
        SYS_UTIMENSAT => fs::utimensat(a[0] as i64, a[1], a[2], a[3]),
        // Cada escrita já vai direto para o disco: não há o que sincronizar.
        SYS_SYNC | SYS_FSYNC | SYS_FDATASYNC => Ok(0),

        // Processos e sinais (proc.rs)
        SYS_FORK | SYS_VFORK => proc::fork(frame),
        SYS_CLONE => proc::clone(frame, a[0], a[1]),
        SYS_EXECVE => match proc::execve(frame, a[0], a[1], a[2]) {
            Ok(()) => return interrupts::finish(frame, None),
            Err(errno) => Err(errno),
        },
        SYS_EXIT | SYS_EXIT_GROUP => {
            process::exit_current(WaitStatus::Exited(a[0] as i32));
            return task::schedule(frame, false);
        }
        SYS_WAIT4 => proc::wait4(a[0] as i64, a[1], a[2]),
        SYS_KILL => proc::kill(a[0] as i64, a[1]),
        SYS_RT_SIGACTION => proc::rt_sigaction(a[0], a[1], a[2]),
        SYS_RT_SIGPROCMASK => proc::rt_sigprocmask(a[0], a[1], a[2]),
        SYS_RT_SIGRETURN => {
            if signal::sigreturn(frame).is_err() {
                task::force_signal_current(signal::SIGSEGV);
            }
            return interrupts::finish(frame, None);
        }
        SYS_GETPID | SYS_GETTID | SYS_SET_TID_ADDRESS => Ok(task::current_ids().0 as i64),
        SYS_GETPPID => Ok(task::current_ids().1 as i64),
        SYS_GETPGRP => Ok(task::current_ids().2 as i64),
        SYS_SETPGID => task::set_process_group(a[0], a[1]).map(|()| 0),
        SYS_GETPGID => proc::getpgid(a[0]),
        SYS_SETSID => task::new_session().map(|sid| sid as i64),
        SYS_GETSID => proc::getsid(a[0]),
        SYS_GETUID | SYS_GETGID | SYS_GETEUID | SYS_GETEGID => Ok(0), // só existe o usuário 0

        // Memória, tempo e outras
        SYS_MMAP => mmap(a[1], a[2], a[3], a[4] as i64),
        SYS_MUNMAP => with_process(|p| p.munmap(a[0], a[1]).map(|()| 0)),
        SYS_BRK => with_process(|p| Ok(p.brk(a[0]) as i64)),
        SYS_SCHED_YIELD => {
            frame.rax = 0;
            return interrupts::finish(frame, Some(false));
        }
        SYS_NANOSLEEP => nanosleep(a[0]),
        SYS_UNAME => uname(a[0]),
        SYS_ARCH_PRCTL => arch_prctl(a[0], a[1]),
        SYS_CLOCK_GETTIME => clock_gettime(a[0], a[1]),
        number => {
            // Registrado só na serial: é o mapa do que falta implementar.
            let _ = writeln!(SERIAL.lock(), "[syscall] pid {}: {number} não implementada", task::current_pid());
            Err(ENOSYS)
        }
    };

    let interrupted = (result == Err(EINTR)).then_some(number);
    frame.rax = match result {
        Ok(value) => value as u64,
        Err(errno) => (-errno) as u64,
    };
    if let Some(next) = signal::deliver(frame, interrupted) {
        return next;
    }
    frame
}

/// Executa `f` com o processo atual (erro se a tarefa não for de usuário).
fn with_process<R>(f: impl FnOnce(&mut process::Process) -> Result<R, Errno>) -> Result<R, Errno> {
    task::with_current_process(f).unwrap_or(Err(EFAULT))
}

/// Valida e devolve um trecho de memória do processo atual.
pub fn user_slice(addr: u64, len: u64, write: bool) -> Result<&'static mut [u8], Errno> {
    if len == 0 {
        return Ok(&mut []);
    }
    with_process(|p| p.check_range(addr, len, write).map(|()| 0))?;
    Ok(unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, len as usize) })
}

pub fn read_u64(addr: u64) -> Result<u64, Errno> {
    let bytes = user_slice(addr, 8, false)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

pub fn write_user(addr: u64, data: &[u8]) -> Result<(), Errno> {
    user_slice(addr, data.len() as u64, true)?.copy_from_slice(data);
    Ok(())
}

/// Lê uma string C (terminada em zero) da memória do usuário.
pub fn read_cstr(addr: u64) -> Result<String, Errno> {
    const MAX: u64 = 4096;
    let mut bytes = Vec::new();
    for offset in 0..MAX {
        let byte = user_slice(addr + offset, 1, false)?[0];
        if byte == 0 {
            return String::from_utf8(bytes).map_err(|_| EINVAL);
        }
        bytes.push(byte);
    }
    Err(ENAMETOOLONG)
}

/// Lê um vetor de strings C terminado em NULL (argv, envp).
pub fn read_cstr_array(addr: u64) -> Result<Vec<String>, Errno> {
    const MAX_ENTRIES: u64 = 1024;
    let mut strings = Vec::new();
    if addr == 0 {
        return Ok(strings);
    }
    for index in 0..MAX_ENTRIES {
        let pointer = read_u64(addr + index * 8)?;
        if pointer == 0 {
            return Ok(strings);
        }
        strings.push(read_cstr(pointer)?);
    }
    Err(E2BIG)
}

fn mmap(len: u64, prot: u64, flags: u64, fd: i64) -> SysResult {
    const MAP_FIXED: u64 = 0x10;
    const MAP_ANONYMOUS: u64 = 0x20;
    if flags & MAP_ANONYMOUS == 0 || fd != -1 {
        return Err(ENODEV); // ainda não há arquivos mapeáveis
    }
    if flags & MAP_FIXED != 0 || prot & !((PROT_READ | PROT_WRITE | PROT_EXEC) as u64) != 0 {
        return Err(EINVAL);
    }
    with_process(|p| p.mmap(len, prot as u32).map(|addr| addr as i64))
}

fn nanosleep(request: u64) -> SysResult {
    let seconds = read_u64(request)?;
    let nanos = read_u64(request + 8)?;
    if nanos >= 1_000_000_000 {
        return Err(EINVAL);
    }
    task::sleep_ms(seconds.saturating_mul(1000).saturating_add(nanos.div_ceil(1_000_000)));
    if task::signal_pending() { Err(EINTR) } else { Ok(0) }
}

fn uname(buf: u64) -> SysResult {
    let out = user_slice(buf, 6 * 65, true)?;
    out.fill(0);
    let fields = ["FerrugemOS", "ferrugem", env!("CARGO_PKG_VERSION"), "#1 (fase 2)", "x86_64", ""];
    for (i, field) in fields.iter().enumerate() {
        out[i * 65..i * 65 + field.len()].copy_from_slice(field.as_bytes());
    }
    Ok(0)
}

fn arch_prctl(code: u64, addr: u64) -> SysResult {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    match code {
        ARCH_SET_FS if addr < USER_TOP => {
            task::set_fs_base(addr);
            Ok(0)
        }
        ARCH_GET_FS => {
            write_user(addr, &task::fs_base().to_le_bytes())?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

fn clock_gettime(clock: u64, out: u64) -> SysResult {
    // REALTIME é a hora de parede (do RTC); os outros contam desde o boot.
    const CLOCKS: [u64; 4] = [0, 1, 4, 7]; // REALTIME, MONOTONIC, MONOTONIC_RAW, BOOTTIME
    if !CLOCKS.contains(&clock) {
        return Err(EINVAL);
    }
    let ms = if clock == 0 { crate::rtc::now_ms() } else { timer::uptime_ms() };
    let mut time = [0u8; 16];
    time[..8].copy_from_slice(&(ms / 1000).to_le_bytes());
    time[8..].copy_from_slice(&((ms % 1000) * 1_000_000).to_le_bytes());
    write_user(out, &time)?;
    Ok(0)
}
