//! Sinais POSIX, com a numeração e a interface do Linux x86_64.
//!
//! Um sinal enviado fica pendente na tarefa. Ele é entregue quando ela está
//! para voltar ao anel 3 (fim de uma syscall ou de uma interrupção): a ação
//! padrão ignora, para ou encerra o processo; um tratador instalado com
//! `rt_sigaction` é chamado montando um quadro na pilha do usuário, e o
//! `rt_sigreturn` (chamado pelo "restorer" ao fim do tratador) desfaz isso.

use core::mem::size_of;

use crate::cpu::FpuState;
use crate::interrupts::InterruptFrame;
use crate::paging::USER_TOP;
use crate::syscall::{self, Errno};
use crate::{gdt, process, task};

pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;
pub const SIGSEGV: u32 = 11;
pub const SIGPIPE: u32 = 13;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
pub const NSIG: u32 = 64;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SA_RESTART: u64 = 0x1000_0000;
pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;

pub const fn bit(signal: u32) -> u64 {
    1 << (signal - 1)
}

/// Sinais que não podem ser bloqueados, ignorados nem tratados.
pub const UNBLOCKABLE: u64 = bit(SIGKILL) | bit(SIGSTOP);
pub const STOP_SIGNALS: u64 = bit(SIGSTOP) | bit(SIGTSTP) | bit(SIGTTIN) | bit(SIGTTOU);

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct SigAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    Terminate,
    Ignore,
    Stop,
    Continue,
}

pub fn default_action(signal: u32) -> DefaultAction {
    match signal {
        SIGCHLD | 23 /* SIGURG */ | 28 /* SIGWINCH */ => DefaultAction::Ignore,
        SIGCONT => DefaultAction::Continue,
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => DefaultAction::Stop,
        _ => DefaultAction::Terminate,
    }
}

pub fn name(signal: u32) -> &'static str {
    const NAMES: [&str; 32] = [
        "?", "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGBUS", "SIGFPE", "SIGKILL", "SIGUSR1",
        "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM", "SIGSTKFLT", "SIGCHLD", "SIGCONT", "SIGSTOP", "SIGTSTP",
        "SIGTTIN", "SIGTTOU", "SIGURG", "SIGXCPU", "SIGXFSZ", "SIGVTALRM", "SIGPROF", "SIGWINCH", "SIGIO", "SIGPWR",
        "SIGSYS",
    ];
    NAMES.get(signal as usize).copied().unwrap_or("SIGRT")
}

/// Tratadores e máscara de bloqueio de um processo.
#[derive(Clone)]
pub struct SignalState {
    pub actions: [SigAction; NSIG as usize],
    pub mask: u64,
}

impl SignalState {
    pub fn new() -> Self {
        SignalState { actions: [SigAction::default(); NSIG as usize], mask: 0 }
    }

    pub fn action(&self, signal: u32) -> SigAction {
        self.actions[signal as usize - 1]
    }

    /// Um sinal com esta ação seria descartado ao ser enviado?
    pub fn ignores(&self, signal: u32) -> bool {
        if bit(signal) & UNBLOCKABLE != 0 {
            return false;
        }
        match self.action(signal).handler {
            SIG_IGN => true,
            SIG_DFL => matches!(default_action(signal), DefaultAction::Ignore | DefaultAction::Continue),
            _ => false,
        }
    }

    /// No `execve`, tratadores deixam de existir (o código deles sumiu);
    /// sinais ignorados continuam ignorados.
    pub fn reset_for_exec(&mut self) {
        for action in self.actions.iter_mut() {
            if action.handler != SIG_IGN {
                *action = SigAction::default();
            }
        }
    }
}

/// O que o kernel salva na pilha do usuário ao chamar um tratador.
#[repr(C)]
struct SignalContext {
    /// r15..rax, na ordem do InterruptFrame.
    registers: [u64; 15],
    rip: u64,
    rflags: u64,
    rsp: u64,
    mask: u64,
    _padding: u64,
    fpu: [u8; 512],
}

const SIGINFO_SIZE: u64 = 128;
const RED_ZONE: u64 = 128;
/// Flags do RFLAGS que o código de usuário pode alterar.
const USER_FLAGS: u64 = 0x0CD5;

fn registers_of(frame: &InterruptFrame) -> [u64; 15] {
    [
        frame.r15, frame.r14, frame.r13, frame.r12, frame.r11, frame.r10, frame.r9, frame.r8, frame.rbp, frame.rdi,
        frame.rsi, frame.rdx, frame.rcx, frame.rbx, frame.rax,
    ]
}

fn set_registers(frame: &mut InterruptFrame, r: &[u64; 15]) {
    [
        frame.r15, frame.r14, frame.r13, frame.r12, frame.r11, frame.r10, frame.r9, frame.r8, frame.rbp, frame.rdi,
        frame.rsi, frame.rdx, frame.rcx, frame.rbx, frame.rax,
    ] = *r;
}

/// Faz a syscall interrompida ser executada de novo ao voltar ao usuário.
fn restart_syscall(frame: &mut InterruptFrame, number: u64) {
    frame.rax = number;
    frame.rip -= 2; // tamanho da instrução `syscall`
}

/// Entrega os sinais pendentes da tarefa atual, que está para voltar ao
/// usuário com `frame`. `interrupted` é o número da syscall que acabou de
/// devolver EINTR, se for o caso. Devolve `Some(frame)` de outra tarefa
/// quando a atual parou ou morreu.
pub fn deliver(frame: &mut InterruptFrame, mut interrupted: Option<u64>) -> Option<*mut InterruptFrame> {
    while let Some((signal, action)) = task::take_pending_signal() {
        match action.handler {
            SIG_IGN => continue,
            SIG_DFL => match default_action(signal) {
                DefaultAction::Ignore | DefaultAction::Continue => continue,
                DefaultAction::Stop => {
                    // Ao ser continuada, a syscall recomeça sozinha.
                    if let Some(number) = interrupted.take() {
                        restart_syscall(frame, number);
                    }
                    task::stop_current(signal);
                    return Some(task::schedule(frame, false));
                }
                DefaultAction::Terminate => {
                    process::exit_current(task::WaitStatus::Signaled(signal));
                    return Some(task::schedule(frame, false));
                }
            },
            _ => {
                if let Some(number) = interrupted.take()
                    && action.flags & SA_RESTART != 0
                {
                    restart_syscall(frame, number);
                }
                if setup_handler(frame, signal, &action).is_err() {
                    // Sem pilha para o tratador: morre como o Linux faria.
                    process::exit_current(task::WaitStatus::Signaled(SIGSEGV));
                    return Some(task::schedule(frame, false));
                }
                return None; // um tratador por vez
            }
        }
    }
    // Interrompida por um sinal que acabou não precisando de tratamento.
    if let Some(number) = interrupted {
        restart_syscall(frame, number);
    }
    None
}

fn setup_handler(frame: &mut InterruptFrame, signal: u32, action: &SigAction) -> Result<(), Errno> {
    let context_size = size_of::<SignalContext>() as u64;
    let context_addr = (frame.rsp.wrapping_sub(RED_ZONE).wrapping_sub(context_size)) & !0xF;
    let info_addr = context_addr - SIGINFO_SIZE;
    let return_addr = info_addr - 8; // ≡ 8 (mod 16), como logo após um `call`

    let old_mask = task::with_current_process(|p| p.signals.mask).unwrap_or(0);
    let mut fpu = FpuState::clean();
    fpu.save();
    let context = SignalContext {
        registers: registers_of(frame),
        rip: frame.rip,
        rflags: frame.rflags,
        rsp: frame.rsp,
        mask: old_mask,
        _padding: 0,
        fpu: *fpu.as_bytes(),
    };
    let mut info = [0u8; SIGINFO_SIZE as usize];
    info[..4].copy_from_slice(&signal.to_le_bytes());

    let stack = syscall::user_slice(return_addr, frame.rsp - RED_ZONE - return_addr, true)?;
    stack[..8].copy_from_slice(&action.restorer.to_le_bytes());
    stack[8..8 + SIGINFO_SIZE as usize].copy_from_slice(&info);
    let context_bytes = unsafe {
        core::slice::from_raw_parts(&context as *const SignalContext as *const u8, context_size as usize)
    };
    let offset = (context_addr - return_addr) as usize;
    stack[offset..offset + context_size as usize].copy_from_slice(context_bytes);

    task::with_current_process(|p| {
        let mut blocked = action.mask;
        if action.flags & SA_NODEFER == 0 {
            blocked |= bit(signal);
        }
        p.signals.mask = (p.signals.mask | blocked) & !UNBLOCKABLE;
        if action.flags & SA_RESETHAND != 0 {
            p.signals.actions[signal as usize - 1] = SigAction::default();
        }
    });

    // O tratador começa com a FPU limpa; a original volta no sigreturn.
    FpuState::clean().restore();
    frame.rip = action.handler;
    frame.rsp = return_addr;
    frame.rdi = signal as u64;
    frame.rsi = info_addr;
    frame.rdx = context_addr;
    frame.rax = 0;
    frame.rflags &= !(1 << 10 | 1 << 8); // DF e TF desligados, como manda a ABI
    Ok(())
}

/// `rt_sigreturn`: restaura o estado salvo por `setup_handler`. Quando o
/// restorer executa `syscall`, o rsp aponta para o siginfo (o `ret` do
/// tratador já tirou o endereço de retorno).
pub fn sigreturn(frame: &mut InterruptFrame) -> Result<(), Errno> {
    let context_addr = frame.rsp + SIGINFO_SIZE;
    let bytes = syscall::user_slice(context_addr, size_of::<SignalContext>() as u64, false)?;
    let context = unsafe { (bytes.as_ptr() as *const SignalContext).read_unaligned() };
    // Um rip fora da metade de usuário faria o próprio `iretq` falhar no
    // anel 0; recusamos antes (o processo morre com SIGSEGV).
    if context.rip >= USER_TOP || context.rsp >= USER_TOP {
        return Err(syscall::EFAULT);
    }

    set_registers(frame, &context.registers);
    frame.rip = context.rip;
    frame.rsp = context.rsp;
    frame.rflags = (context.rflags & USER_FLAGS) | 0x202;
    frame.cs = gdt::USER_CODE as u64;
    frame.ss = gdt::USER_DATA as u64;
    task::with_current_process(|p| p.signals.mask = context.mask & !UNBLOCKABLE);
    FpuState::from_bytes(&context.fpu).restore();
    Ok(())
}
