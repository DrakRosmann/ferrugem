//! Syscalls de processos, sinais e grupos de processos.

use alloc::string::String;
use alloc::vec::Vec;

use super::{
    EINTR, EINVAL, ENOSYS, Errno, SysResult, read_cstr, read_cstr_array, read_u64, user_slice, with_process, write_user,
};
use crate::interrupts::InterruptFrame;
use crate::process;
use crate::signal::{NSIG, SigAction, UNBLOCKABLE, bit};
use crate::task::{self, Pid, WaitTarget};

pub fn fork(frame: &InterruptFrame) -> SysResult {
    let child = with_process(|p| p.fork())?;
    Ok(task::fork_current(frame, child) as i64)
}

/// Só a forma de `clone` usada pelo `fork` das libcs: sem compartilhar
/// memória nem nada mais (os bits baixos são o sinal enviado ao pai).
pub fn clone(frame: &InterruptFrame, flags: u64, stack: u64) -> SysResult {
    if flags & !0xFF != 0 || stack != 0 {
        return Err(ENOSYS);
    }
    fork(frame)
}

pub fn execve(frame: &mut InterruptFrame, path: u64, argv: u64, envp: u64) -> Result<(), Errno> {
    let path = read_cstr(path)?;
    let args = read_cstr_array(argv)?;
    let env = read_cstr_array(envp)?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let env: Vec<&str> = env.iter().map(String::as_str).collect();
    process::exec(frame, &path, &args, &env)
}

pub fn wait4(pid: i64, status: u64, options: u64) -> SysResult {
    let target = match pid as i32 {
        -1 => WaitTarget::Any,
        0 => WaitTarget::Group(0),
        p if p > 0 => WaitTarget::Pid(p as Pid),
        group => WaitTarget::Group(group.unsigned_abs() as Pid),
    };
    if status != 0 {
        user_slice(status, 4, true)?;
    }
    match task::wait_child(target, options as u32)? {
        None => Ok(0),
        Some((pid, code)) => {
            if status != 0 {
                write_user(status, &code.to_le_bytes())?;
            }
            Ok(pid as i64)
        }
    }
}

pub fn kill(pid: i64, signal: u64) -> SysResult {
    if signal > NSIG as u64 {
        return Err(EINVAL);
    }
    task::kill(pid as i32 as i64, signal as u32).map(|()| 0)
}

/// O `struct sigaction` do kernel: tratador, flags, restorer, máscara.
fn read_action(addr: u64) -> Result<SigAction, Errno> {
    Ok(SigAction {
        handler: read_u64(addr)?,
        flags: read_u64(addr + 8)?,
        restorer: read_u64(addr + 16)?,
        mask: read_u64(addr + 24)?,
    })
}

pub fn rt_sigaction(signal: u64, new: u64, old: u64) -> SysResult {
    let signal = signal as u32;
    if signal == 0 || signal > NSIG {
        return Err(EINVAL);
    }
    let new = if new != 0 { Some(read_action(new)?) } else { None };
    if new.is_some() && bit(signal) & UNBLOCKABLE != 0 {
        return Err(EINVAL);
    }
    let (previous, now_ignored) = with_process(|p| {
        let previous = p.signals.action(signal);
        if let Some(action) = new {
            p.signals.actions[signal as usize - 1] = SigAction { mask: action.mask & !UNBLOCKABLE, ..action };
        }
        Ok((previous, p.signals.ignores(signal)))
    })?;
    if now_ignored {
        task::clear_pending(signal); // sinal que passa a ser ignorado some da fila
    }
    if old != 0 {
        let mut bytes = [0u8; 32];
        for (i, value) in [previous.handler, previous.flags, previous.restorer, previous.mask].iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        write_user(old, &bytes)?;
    }
    Ok(0)
}

pub fn rt_sigprocmask(how: u64, set: u64, old: u64) -> SysResult {
    const SIG_BLOCK: u64 = 0;
    const SIG_UNBLOCK: u64 = 1;
    const SIG_SETMASK: u64 = 2;
    let current = with_process(|p| Ok(p.signals.mask))?;
    if set != 0 {
        let set = read_u64(set)?;
        let mask = match how {
            SIG_BLOCK => current | set,
            SIG_UNBLOCK => current & !set,
            SIG_SETMASK => set,
            _ => return Err(EINVAL),
        };
        with_process(|p| {
            p.signals.mask = mask & !UNBLOCKABLE;
            Ok(())
        })?;
    }
    if old != 0 {
        write_user(old, &current.to_le_bytes())?;
    }
    // Sinais desbloqueados agora são entregues ao fim desta syscall.
    Ok(0)
}

pub fn getpgid(pid: u64) -> SysResult {
    let pid = pid as i32;
    if pid == 0 {
        return Ok(task::current_ids().2 as i64);
    }
    task::ids_of(pid as Pid).map(|(pgid, _)| pgid as i64)
}

pub fn getsid(pid: u64) -> SysResult {
    let pid = pid as i32;
    if pid == 0 {
        return Ok(task::current_ids().3 as i64);
    }
    task::ids_of(pid as Pid).map(|(_, sid)| sid as i64)
}

/// `rt_sigsuspend`: troca a máscara de sinais e dorme até chegar um sinal
/// que precise ser entregue. Sempre volta com EINTR.
pub fn rt_sigsuspend(mask: u64) -> SysResult {
    let mask = read_u64(mask)?;
    with_process(|p| {
        p.signals.suspended_mask = Some(p.signals.mask);
        p.signals.mask = mask & !UNBLOCKABLE;
        Ok(())
    })?;
    while !task::signal_pending() {
        task::sleep_ms(60 * 60 * 1000); // um sinal acorda antes
    }
    Err(EINTR)
}
