//! Tarefas e escalonador preemptivo (round-robin acionado pelo timer).
//!
//! Há dois tipos de tarefa: threads do kernel (anel 0, espaço de
//! endereçamento do kernel) e processos de usuário (anel 3, com seu próprio
//! espaço). Toda tarefa tem uma pilha de kernel, e trocar de tarefa é só
//! devolver ao isr.s o `InterruptFrame` salvo de outra: o `iretq` a retoma,
//! no anel em que ela estava. Junto troca-se CR3, a FPU e o rsp0 do TSS.
//!
//! Também mora aqui o que depende da lista de tarefas: parentesco (pai,
//! grupo de processos, sessão), processos zumbis e `wait4`, e o envio de
//! sinais.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::mem::size_of;

use crate::cpu::{self, FpuState};
use crate::gdt;
use crate::interrupts::{InterruptFrame, YIELD_VECTOR};
use crate::paging;
use crate::process::Process;
use crate::signal::{self, SIGCHLD, SIGCONT, SIGINT, SIGKILL, SIGTERM, STOP_SIGNALS, SigAction, UNBLOCKABLE, bit};
use crate::sync::{SpinLock, WaitQueue};
use crate::syscall::{ECHILD, EINTR, EPERM, ESRCH, Errno};
use crate::{syscall, timer};

pub type Pid = u64;

const KERNEL_STACK_SIZE: usize = 64 * 1024;
const MSR_FS_BASE: u32 = 0xC000_0100;
/// Quantos zumbis órfãos (filhos do kernel) guardar sem ninguém esperar.
const MAX_ORPHAN_ZOMBIES: usize = 64;
pub const KERNEL_PID: Pid = 0;
pub const IDLE_PID: Pid = 1;

pub const WNOHANG: u32 = 1;
pub const WUNTRACED: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Ready,
    Sleeping { until: u64 },
    /// Bloqueada até a geração da fila (um `*const WaitQueue`) mudar.
    Blocked { queue: usize, seen: u64 },
    /// Parada por SIGSTOP/SIGTSTP até receber SIGCONT.
    Stopped,
    Dead,
}

/// Como um processo terminou, no formato de `wait4`.
#[derive(Clone, Copy)]
pub enum WaitStatus {
    Exited(i32),
    Signaled(u32),
}

impl WaitStatus {
    pub fn encode(self) -> u32 {
        match self {
            WaitStatus::Exited(code) => ((code as u32) & 0xFF) << 8,
            WaitStatus::Signaled(signal) => signal & 0x7F,
        }
    }
}

struct Task {
    pid: Pid,
    parent: Pid,
    pgid: Pid,
    sid: Pid,
    name: String,
    state: State,
    frame: *mut InterruptFrame,
    /// `None` para a tarefa 0, que usa a pilha do boot.
    _kernel_stack: Option<Box<[u8]>>,
    kernel_stack_top: u64,
    fpu: Box<FpuState>,
    /// Base do segmento FS (TLS) do processo, definida por `arch_prctl`.
    fs_base: u64,
    process: Option<Process>,
    cpu_ticks: u64,
    /// Sinais pendentes (bit n-1 = sinal n).
    pending: u64,
    /// Sinal que parou a tarefa, ainda não informado ao pai por `wait4`.
    stop_report: Option<u32>,
}

unsafe impl Send for Task {}

impl Task {
    fn cr3(&self) -> u64 {
        self.process.as_ref().map_or(paging::kernel_p4(), |p| p.space.p4())
    }

    fn alive(&self) -> bool {
        self.state != State::Dead
    }

    /// Sinais pendentes que não estão bloqueados.
    fn deliverable(&self) -> u64 {
        match &self.process {
            Some(p) => self.pending & !(p.signals.mask & !UNBLOCKABLE),
            None => 0,
        }
    }
}

/// Um processo que terminou e espera o pai recolher seu estado.
struct Zombie {
    pid: Pid,
    parent: Pid,
    pgid: Pid,
    sid: Pid,
    status: u32,
}

struct Scheduler {
    tasks: Vec<Task>,
    current: usize,
    next_pid: Pid,
    zombies: Vec<Zombie>,
}

static SCHEDULER: SpinLock<Scheduler> =
    SpinLock::new(Scheduler { tasks: Vec::new(), current: 0, next_pid: 0, zombies: Vec::new() });

/// Acordada quando um filho termina ou para (para quem está em `wait4`).
static CHILD_EVENTS: WaitQueue = WaitQueue::new();

pub struct TaskInfo {
    pub pid: Pid,
    pub parent: Pid,
    pub pgid: Pid,
    pub name: String,
    pub state: State,
    pub cpu_ms: u64,
    pub user: bool,
    pub memory: u64,
}

/// Transforma o fluxo atual (o kernel_main) na tarefa 0 e cria a ociosa.
pub fn init() {
    {
        let mut sched = SCHEDULER.lock();
        sched.tasks.push(Task {
            pid: KERNEL_PID,
            parent: KERNEL_PID,
            pgid: KERNEL_PID,
            sid: KERNEL_PID,
            name: "monitor".to_string(),
            state: State::Ready,
            frame: core::ptr::null_mut(),
            _kernel_stack: None,
            kernel_stack_top: 0,
            fpu: Box::new(FpuState::clean()),
            fs_base: 0,
            process: None,
            cpu_ticks: 0,
            pending: 0,
            stop_report: None,
        });
        sched.next_pid = 1;
    }
    spawn("ocioso", || {
        loop {
            cpu::sti_hlt();
        }
    });
}

/// Reserva uma pilha de kernel e devolve (pilha, topo alinhado a 16).
fn new_kernel_stack() -> (Box<[u8]>, u64) {
    let mut stack = vec![0u8; KERNEL_STACK_SIZE].into_boxed_slice();
    let top = (stack.as_mut_ptr() as u64 + KERNEL_STACK_SIZE as u64) & !0xF;
    (stack, top)
}

struct NewTask {
    name: String,
    parent: Pid,
    /// `None`: a tarefa lidera um grupo e uma sessão novos.
    ids: Option<(Pid, Pid)>,
    stack: Box<[u8]>,
    top: u64,
    frame: *mut InterruptFrame,
    fpu: Box<FpuState>,
    fs_base: u64,
    process: Option<Process>,
}

fn add_task(new: NewTask) -> Pid {
    let mut sched = SCHEDULER.lock();
    let pid = sched.next_pid;
    sched.next_pid += 1;
    let (pgid, sid) = new.ids.unwrap_or((pid, pid));
    sched.tasks.push(Task {
        pid,
        parent: new.parent,
        pgid,
        sid,
        name: new.name,
        state: State::Ready,
        frame: new.frame,
        _kernel_stack: Some(new.stack),
        kernel_stack_top: new.top,
        fpu: new.fpu,
        fs_base: new.fs_base,
        process: new.process,
        cpu_ticks: 0,
        pending: 0,
        stop_report: None,
    });
    pid
}

/// Cria uma thread do kernel que executa `entry`.
pub fn spawn<F: FnOnce() + Send + 'static>(name: &str, entry: F) -> Pid {
    let closure: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(entry));
    let (stack, top) = new_kernel_stack();

    // Prepara um frame como se a tarefa tivesse sido interrompida logo na
    // entrada de `trampoline`, com rsp ≡ 8 (mod 16) como após um `call`.
    let entry_rsp = top - 8;
    let frame = ((entry_rsp - size_of::<InterruptFrame>() as u64) & !0xF) as *mut InterruptFrame;
    unsafe {
        (entry_rsp as *mut u64).write(0); // endereço de retorno falso
        frame.write(InterruptFrame {
            rip: trampoline as *const () as u64,
            cs: gdt::KERNEL_CODE as u64,
            rflags: 0x202, // interrupções habilitadas
            rsp: entry_rsp,
            ss: gdt::KERNEL_DATA as u64,
            rdi: Box::into_raw(closure) as u64,
            ..InterruptFrame::default()
        });
    }
    add_task(NewTask {
        name: name.to_string(),
        parent: KERNEL_PID,
        ids: Some((KERNEL_PID, KERNEL_PID)),
        stack,
        top,
        frame,
        fpu: Box::new(FpuState::clean()),
        fs_base: 0,
        process: None,
    })
}

extern "C" fn trampoline(closure: *mut Box<dyn FnOnce() + Send>) -> ! {
    let closure = unsafe { Box::from_raw(closure) };
    closure();
    exit()
}

/// Cria um processo de usuário que começa em `entry`, no anel 3, com a
/// pilha de usuário em `user_rsp`. Ele é filho do kernel e lidera seu grupo
/// e sua sessão.
pub fn spawn_user(name: &str, process: Process, entry: u64, user_rsp: u64) -> Pid {
    let (stack, top) = new_kernel_stack();
    let frame = (top - size_of::<InterruptFrame>() as u64) as *mut InterruptFrame;
    unsafe {
        frame.write(InterruptFrame {
            rip: entry,
            cs: gdt::USER_CODE as u64,
            rflags: 0x202,
            rsp: user_rsp,
            ss: gdt::USER_DATA as u64,
            ..InterruptFrame::default()
        });
    }
    add_task(NewTask {
        name: name.to_string(),
        parent: KERNEL_PID,
        ids: None,
        stack,
        top,
        frame,
        fpu: Box::new(FpuState::clean()),
        fs_base: 0,
        process: Some(process),
    })
}

/// `fork`: cria o filho da tarefa atual, que volta da mesma syscall com 0.
pub fn fork_current(frame: &InterruptFrame, process: Process) -> Pid {
    let (stack, top) = new_kernel_stack();
    let child_frame = (top - size_of::<InterruptFrame>() as u64) as *mut InterruptFrame;
    unsafe { child_frame.write(InterruptFrame { rax: 0, ..frame.clone() }) };
    // A FPU do pai está viva nos registradores (o kernel não a usa).
    let mut fpu = Box::new(FpuState::clean());
    fpu.save();
    let (name, parent, pgid, sid, fs_base) = {
        let sched = SCHEDULER.lock();
        let parent = &sched.tasks[sched.current];
        (parent.name.clone(), parent.pid, parent.pgid, parent.sid, parent.fs_base)
    };
    add_task(NewTask {
        name,
        parent,
        ids: Some((pgid, sid)),
        stack,
        top,
        frame: child_frame,
        fpu,
        fs_base,
        process: Some(process),
    })
}

/// Depois de um `execve` bem-sucedido: novo nome e TLS zerado.
pub fn exec_current(name: &str) {
    let mut sched = SCHEDULER.lock();
    let current = sched.current;
    let task = &mut sched.tasks[current];
    task.name = name.to_string();
    task.fs_base = 0;
    unsafe { cpu::wrmsr(MSR_FS_BASE, 0) };
}

/// Chamado pelo despachante de interrupções. Salva o frame da tarefa atual
/// e devolve o da próxima a executar.
pub fn schedule(frame: *mut InterruptFrame, from_timer: bool) -> *mut InterruptFrame {
    let mut sched = SCHEDULER.lock();
    if sched.tasks.is_empty() {
        return frame;
    }
    let current = sched.current;
    sched.tasks[current].frame = frame;
    if from_timer {
        sched.tasks[current].cpu_ticks += 1;
    }

    // Libera tarefas mortas (exceto a atual, cuja pilha ainda estamos usando).
    let current_pid = sched.tasks[current].pid;
    sched.tasks.retain(|t| t.alive() || t.pid == current_pid);
    let current = sched.tasks.iter().position(|t| t.pid == current_pid).unwrap();

    let now = timer::ticks();
    for task in sched.tasks.iter_mut() {
        match task.state {
            State::Sleeping { until } if now >= until => task.state = State::Ready,
            State::Blocked { queue, seen } => {
                // A fila vive enquanto a tarefa bloqueada a referencia.
                if unsafe { &*(queue as *const WaitQueue) }.generation() != seen {
                    task.state = State::Ready;
                }
            }
            _ => {}
        }
    }

    let count = sched.tasks.len();
    let next = (1..=count)
        .map(|offset| (current + offset) % count)
        .find(|&i| sched.tasks[i].state == State::Ready && sched.tasks[i].pid != IDLE_PID)
        .or_else(|| sched.tasks.iter().position(|t| t.pid == IDLE_PID))
        .unwrap_or(current);

    if next != current {
        if sched.tasks[current].alive() {
            sched.tasks[current].fpu.save();
        }
        let old_cr3 = sched.tasks[current].cr3();
        let old_fs = sched.tasks[current].fs_base;
        let task = &sched.tasks[next];
        task.fpu.restore();
        let cr3 = task.cr3();
        if cr3 != old_cr3 {
            unsafe { cpu::write_cr3(cr3) };
        }
        if task.fs_base != old_fs {
            unsafe { cpu::wrmsr(MSR_FS_BASE, task.fs_base) };
        }
        gdt::set_kernel_stack(task.kernel_stack_top);
        syscall::set_kernel_stack(task.kernel_stack_top);
    }

    sched.current = next;
    sched.tasks[next].frame
}

pub fn yield_now() {
    unsafe { asm!("int {}", const YIELD_VECTOR) }
}

/// Dorme `ms` milissegundos. Um sinal acorda o processo antes da hora.
pub fn sleep_ms(ms: u64) {
    {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if sched.tasks[current].deliverable() != 0 {
            return;
        }
        sched.tasks[current].state = State::Sleeping { until: timer::ticks() + timer::ms_to_ticks(ms) };
    }
    yield_now();
}

/// Bloqueia até a geração de `queue` deixar de ser `seen` (ou chegar um
/// sinal). Quem chama lê a geração *antes* de testar sua condição, então um
/// aviso que chegue no meio do caminho nunca é perdido.
pub fn block_on(queue: &WaitQueue, seen: u64) {
    {
        let mut sched = SCHEDULER.lock();
        let current = sched.current;
        if sched.tasks.is_empty() {
            drop(sched);
            cpu::sti_hlt();
            return;
        }
        if queue.generation() != seen || sched.tasks[current].deliverable() != 0 {
            return;
        }
        sched.tasks[current].state = State::Blocked { queue: queue as *const WaitQueue as usize, seen };
    }
    yield_now();
}

impl Scheduler {
    fn index_of(&self, pid: Pid) -> Option<usize> {
        self.tasks.iter().position(|t| t.pid == pid && t.alive())
    }

    fn current_task(&mut self) -> &mut Task {
        let current = self.current;
        &mut self.tasks[current]
    }

    fn mark_dead(&mut self, index: usize, status: WaitStatus) {
        let task = &mut self.tasks[index];
        task.state = State::Dead;
        let (pid, parent, pgid, sid) = (task.pid, task.parent, task.pgid, task.sid);

        // Os filhos órfãos passam a ser do kernel.
        for task in self.tasks.iter_mut().filter(|t| t.parent == pid) {
            task.parent = KERNEL_PID;
        }
        for zombie in self.zombies.iter_mut().filter(|z| z.parent == pid) {
            zombie.parent = KERNEL_PID;
        }
        self.zombies.push(Zombie { pid, parent, pgid, sid, status: status.encode() });
        while self.zombies.iter().filter(|z| z.parent == KERNEL_PID).count() > MAX_ORPHAN_ZOMBIES {
            let oldest = self.zombies.iter().position(|z| z.parent == KERNEL_PID).unwrap();
            self.zombies.remove(oldest);
        }
        CHILD_EVENTS.wake_all();
        if let Some(parent) = self.index_of(parent) {
            self.signal(parent, SIGCHLD);
        }
    }

    /// Envia `signal` à tarefa `index`, com as regras de descarte do Unix.
    fn signal(&mut self, index: usize, signal: u32) {
        let task = &mut self.tasks[index];
        if !task.alive() {
            return;
        }
        let Some(process) = &task.process else {
            // Threads do kernel não tratam sinais: os de término as encerram.
            if matches!(signal, SIGKILL | SIGTERM | SIGINT) && task.pid > IDLE_PID {
                self.mark_dead(index, WaitStatus::Signaled(signal));
            }
            return;
        };
        let ignored = process.signals.ignores(signal);
        let blocked = process.signals.mask & !UNBLOCKABLE & bit(signal) != 0;

        if signal == SIGCONT {
            task.pending &= !STOP_SIGNALS;
            if task.state == State::Stopped {
                task.state = State::Ready;
                task.stop_report = None;
            }
        } else if bit(signal) & STOP_SIGNALS != 0 {
            task.pending &= !bit(SIGCONT);
        }
        if ignored {
            return;
        }
        task.pending |= bit(signal);
        let wake = match task.state {
            State::Stopped => signal == SIGKILL,
            State::Blocked { .. } | State::Sleeping { .. } => !blocked,
            _ => false,
        };
        if wake {
            task.state = State::Ready;
        }
    }
}

/// Há sinal pendente e não bloqueado para a tarefa atual? As esperas
/// interrompíveis consultam isto para devolver EINTR.
pub fn signal_pending() -> bool {
    SCHEDULER.lock().current_task().deliverable() != 0
}

/// Retira o sinal pendente de menor número e devolve sua ação.
pub fn take_pending_signal() -> Option<(u32, SigAction)> {
    let mut sched = SCHEDULER.lock();
    let task = sched.current_task();
    let deliverable = task.deliverable();
    if deliverable == 0 {
        return None;
    }
    let signal = deliverable.trailing_zeros() + 1;
    task.pending &= !bit(signal);
    let action = task.process.as_ref()?.signals.action(signal);
    Some((signal, action))
}

/// Retira `signal` dos pendentes da tarefa atual.
pub fn clear_pending(signal: u32) {
    SCHEDULER.lock().current_task().pending &= !bit(signal);
}

pub fn send_signal_current(signal: u32) {
    let mut sched = SCHEDULER.lock();
    let current = sched.current;
    sched.signal(current, signal);
}

/// Sinal gerado por uma exceção da CPU: se estiver bloqueado ou ignorado,
/// volta à ação padrão (senão o processo repetiria a falha para sempre).
pub fn force_signal_current(signal: u32) {
    let mut sched = SCHEDULER.lock();
    let task = sched.current_task();
    if let Some(process) = &mut task.process {
        let state = &mut process.signals;
        if state.mask & bit(signal) != 0 || state.action(signal).handler == signal::SIG_IGN {
            state.mask &= !bit(signal);
            state.actions[signal as usize - 1] = SigAction::default();
        }
        task.pending |= bit(signal);
    }
}

/// `kill(pid, sinal)`: pid > 0 é um processo, 0 o grupo de quem chama,
/// -1 todos os processos, < -1 o grupo -pid. Sinal 0 só testa a existência.
pub fn kill(target: i64, signal: u32) -> Result<(), Errno> {
    let mut sched = SCHEDULER.lock();
    let me = sched.tasks[sched.current].pid;
    let my_group = sched.tasks[sched.current].pgid;
    let targets: Vec<usize> = (0..sched.tasks.len())
        .filter(|&i| {
            let t = &sched.tasks[i];
            t.alive()
                && t.process.is_some()
                && match target {
                    0 => t.pgid == my_group,
                    -1 => t.pid != me,
                    pid if pid > 0 => t.pid == pid as Pid,
                    group => t.pgid == group.unsigned_abs(),
                }
        })
        .collect();
    if targets.is_empty() {
        return Err(if target > 0 && sched.index_of(target as Pid).is_some() { EPERM } else { ESRCH });
    }
    if signal != 0 {
        for index in targets {
            sched.signal(index, signal);
        }
    }
    Ok(())
}

/// Envia um sinal a todo um grupo (usado pelo terminal: Ctrl+C, Ctrl+Z).
pub fn signal_group(pgid: Pid, signal: u32) {
    let _ = kill(-(pgid as i64), signal);
}

pub enum KillError {
    NotFound,
    Protected,
}

/// O `matar` do monitor: SIGKILL para processos, fim para threads do kernel.
pub fn terminate(pid: Pid) -> Result<(), KillError> {
    if pid == KERNEL_PID || pid == IDLE_PID {
        return Err(KillError::Protected);
    }
    let mut sched = SCHEDULER.lock();
    let index = sched.index_of(pid).ok_or(KillError::NotFound)?;
    sched.signal(index, SIGKILL);
    Ok(())
}

/// Continua uma tarefa parada (o `fg` do monitor).
pub fn resume(pid: Pid) -> Result<(), KillError> {
    let mut sched = SCHEDULER.lock();
    let index = sched.index_of(pid).ok_or(KillError::NotFound)?;
    sched.signal(index, SIGCONT);
    Ok(())
}

/// Para a tarefa atual por causa de `signal` e avisa o pai.
pub fn stop_current(signal: u32) {
    let mut sched = SCHEDULER.lock();
    let task = sched.current_task();
    task.state = State::Stopped;
    task.stop_report = Some(signal);
    let parent = task.parent;
    CHILD_EVENTS.wake_all();
    if let Some(parent) = sched.index_of(parent) {
        sched.signal(parent, SIGCHLD);
    }
}

/// Marca a tarefa atual como encerrada. Quem chama precisa em seguida
/// ceder a CPU (ou devolver `schedule` ao despachante).
pub fn exit_current(status: WaitStatus) {
    let mut sched = SCHEDULER.lock();
    let current = sched.current;
    sched.mark_dead(current, status);
}

/// Encerra a thread do kernel atual.
pub fn exit() -> ! {
    exit_current(WaitStatus::Exited(0));
    yield_now();
    unreachable!("tarefa morta foi escalonada");
}

/// Qual filho `wait4` deve esperar.
#[derive(Clone, Copy)]
pub enum WaitTarget {
    Any,
    Pid(Pid),
    Group(Pid),
}

/// `wait4`: espera um filho da tarefa atual terminar (ou parar, com
/// WUNTRACED) e devolve (pid, estado codificado). `None` com WNOHANG.
pub fn wait_child(target: WaitTarget, options: u32) -> Result<Option<(Pid, u32)>, Errno> {
    loop {
        let seen = CHILD_EVENTS.generation();
        {
            let mut sched = SCHEDULER.lock();
            let me = sched.tasks[sched.current].pid;
            let my_group = sched.tasks[sched.current].pgid;
            let wanted = |pid: Pid, pgid: Pid| match target {
                WaitTarget::Any => true,
                WaitTarget::Pid(p) => pid == p,
                WaitTarget::Group(0) => pgid == my_group,
                WaitTarget::Group(g) => pgid == g,
            };
            if let Some(i) = sched.zombies.iter().position(|z| z.parent == me && wanted(z.pid, z.pgid)) {
                let zombie = sched.zombies.remove(i);
                return Ok(Some((zombie.pid, zombie.status)));
            }
            let mut has_child = false;
            for task in sched.tasks.iter_mut().filter(|t| t.alive() && t.parent == me && wanted(t.pid, t.pgid)) {
                has_child = true;
                if options & WUNTRACED != 0
                    && let Some(signal) = task.stop_report.take()
                {
                    return Ok(Some((task.pid, signal << 8 | 0x7F)));
                }
            }
            if !has_child {
                return Err(ECHILD);
            }
        }
        if options & WNOHANG != 0 {
            return Ok(None);
        }
        if signal_pending() {
            return Err(EINTR);
        }
        block_on(&CHILD_EVENTS, seen);
    }
}

/// (pid, pai, grupo, sessão) da tarefa atual.
pub fn current_ids() -> (Pid, Pid, Pid, Pid) {
    let sched = SCHEDULER.lock();
    let t = &sched.tasks[sched.current];
    (t.pid, t.parent, t.pgid, t.sid)
}

/// (grupo, sessão) de outra tarefa.
pub fn ids_of(pid: Pid) -> Result<(Pid, Pid), Errno> {
    let sched = SCHEDULER.lock();
    let index = sched.index_of(pid).ok_or(ESRCH)?;
    Ok((sched.tasks[index].pgid, sched.tasks[index].sid))
}

/// `setpgid`: move `pid` (a própria tarefa ou um filho, na mesma sessão)
/// para o grupo `pgid`, que precisa ser novo (= pid) ou existir na sessão.
pub fn set_process_group(pid: Pid, pgid: Pid) -> Result<(), Errno> {
    let mut sched = SCHEDULER.lock();
    let (me, my_session) = (sched.tasks[sched.current].pid, sched.tasks[sched.current].sid);
    let target_pid = if pid == 0 { me } else { pid };
    let pgid = if pgid == 0 { target_pid } else { pgid };
    let index = sched.index_of(target_pid).ok_or(ESRCH)?;
    let target = &sched.tasks[index];
    if target.pid != me && target.parent != me {
        return Err(ESRCH);
    }
    if target.sid != my_session || target.pid == target.sid {
        return Err(EPERM);
    }
    if pgid != target_pid && !group_exists(&sched, pgid, my_session) {
        return Err(EPERM);
    }
    sched.tasks[index].pgid = pgid;
    Ok(())
}

/// Como no Linux, um zumbi ainda não esperado continua no seu grupo: sem
/// isso, em `true | cat` o `cat` poderia não achar o grupo do `true`.
fn group_exists(sched: &Scheduler, pgid: Pid, sid: Pid) -> bool {
    sched.tasks.iter().any(|t| t.alive() && t.pgid == pgid && t.sid == sid)
        || sched.zombies.iter().any(|z| z.pgid == pgid && z.sid == sid)
}

/// O grupo `pgid` existe na sessão `sid`? (para `tcsetpgrp`)
pub fn group_in_session(pgid: Pid, sid: Pid) -> bool {
    group_exists(&SCHEDULER.lock(), pgid, sid)
}

/// `setsid`: a tarefa atual vira líder de uma sessão e um grupo novos.
pub fn new_session() -> Result<Pid, Errno> {
    let mut sched = SCHEDULER.lock();
    let task = sched.current_task();
    if task.pgid == task.pid {
        return Err(EPERM); // já lidera um grupo
    }
    task.pgid = task.pid;
    task.sid = task.pid;
    Ok(task.pid)
}

/// Executa `f` com o processo da tarefa atual (se ela for de usuário).
pub fn with_current_process<R>(f: impl FnOnce(&mut Process) -> R) -> Option<R> {
    let mut sched = SCHEDULER.lock();
    sched.current_task().process.as_mut().map(f)
}

pub fn set_fs_base(base: u64) {
    SCHEDULER.lock().current_task().fs_base = base;
    unsafe { cpu::wrmsr(MSR_FS_BASE, base) };
}

pub fn fs_base() -> u64 {
    SCHEDULER.lock().current_task().fs_base
}

pub fn current_pid() -> Pid {
    let sched = SCHEDULER.lock();
    sched.tasks.get(sched.current).map_or(KERNEL_PID, |t| t.pid)
}

pub fn current_is_dead() -> bool {
    let sched = SCHEDULER.lock();
    sched.tasks.get(sched.current).is_some_and(|t| !t.alive())
}

pub fn current_is_user() -> bool {
    let sched = SCHEDULER.lock();
    sched.tasks.get(sched.current).is_some_and(|t| t.process.is_some())
}

pub fn list() -> Vec<TaskInfo> {
    let sched = SCHEDULER.lock();
    sched
        .tasks
        .iter()
        .filter(|t| t.alive())
        .map(|t| TaskInfo {
            pid: t.pid,
            parent: t.parent,
            pgid: t.pgid,
            name: t.name.clone(),
            state: t.state,
            cpu_ms: t.cpu_ticks * 1000 / timer::HZ,
            user: t.process.is_some(),
            memory: t.process.as_ref().map_or(0, |p| p.space.resident_bytes()),
        })
        .collect()
}

pub fn count() -> usize {
    SCHEDULER.lock().tasks.iter().filter(|t| t.alive()).count()
}
