//! IDT, controlador de interrupções 8259 (PIC) e o despachante central.

use core::arch::{asm, global_asm};
use core::mem::size_of;
use core::ptr::addr_of_mut;

use crate::cpu::{self, inb, io_wait, outb};
use crate::console;
use crate::gdt;
use crate::paging::USER_TOP;
use crate::vga::Color;
use crate::{keyboard, process, serial, signal, syscall, task, timer};

global_asm!(include_str!("isr.s"), options(att_syntax, raw));

unsafe extern "C" {
    static isr_stubs: u8;
}

pub const IRQ_BASE: u8 = 32;
pub const IRQ_TIMER: u8 = IRQ_BASE;
pub const IRQ_KEYBOARD: u8 = IRQ_BASE + 1;
pub const IRQ_COM1: u8 = IRQ_BASE + 4;
/// Interrupção de software usada por uma tarefa para ceder a CPU.
pub const YIELD_VECTOR: u8 = 0x81;
/// "Vetor" que a entrada de `syscall` grava no frame. Fica fora de 0..=255
/// para nunca se confundir com uma interrupção de verdade.
pub const SYSCALL_MARK: u64 = 0x100;

/// Estado salvo por isr.s; a ordem espelha os `push` (o último fica no topo).
#[repr(C)]
#[derive(Default, Clone)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    // Empilhado pela CPU:
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl InterruptFrame {
    pub fn from_user(&self) -> bool {
        self.cs & 3 == 3
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}

impl IdtEntry {
    const EMPTY: Self =
        Self { offset_low: 0, selector: 0, ist: 0, attributes: 0, offset_mid: 0, offset_high: 0, _reserved: 0 };

    fn new(handler: u64, ist: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector: gdt::KERNEL_CODE,
            ist,
            attributes: 0x8E, // presente, anel 0, porta de interrupção
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            _reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::EMPTY; 256];

pub fn init() {
    let stubs = &raw const isr_stubs as u64;
    unsafe {
        let idt = &mut *addr_of_mut!(IDT);
        for (vector, entry) in idt.iter_mut().enumerate() {
            let ist = if vector == 8 { gdt::DOUBLE_FAULT_IST } else { 0 };
            *entry = IdtEntry::new(stubs + 16 * vector as u64, ist);
        }
        let pointer = IdtPointer { limit: (size_of::<[IdtEntry; 256]>() - 1) as u16, base: idt.as_ptr() as u64 };
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
    pic::init();
}

const EXCEPTIONS: [&str; 32] = [
    "divisão por zero",
    "depuração",
    "NMI",
    "ponto de parada",
    "overflow",
    "limite excedido",
    "instrução inválida",
    "dispositivo indisponível",
    "falta dupla",
    "segmento de coprocessador",
    "TSS inválido",
    "segmento ausente",
    "falha de segmento de pilha",
    "proteção geral",
    "falha de página",
    "reservada",
    "erro de ponto flutuante x87",
    "verificação de alinhamento",
    "verificação de máquina",
    "erro de ponto flutuante SIMD",
    "virtualização",
    "proteção de controle",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "injeção de hipervisor",
    "comunicação VMM",
    "segurança",
    "reservada",
];

#[unsafe(no_mangle)]
extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) -> *mut InterruptFrame {
    if frame.vector == SYSCALL_MARK {
        return syscall::dispatch(frame);
    }
    let vector = frame.vector as u8;
    let mut reschedule = None;
    match vector {
        0..=31 => return exception(frame),
        IRQ_TIMER => {
            timer::tick();
            pic::eoi(0);
            reschedule = Some(true);
        }
        IRQ_KEYBOARD => {
            keyboard::handle_irq();
            pic::eoi(1);
        }
        IRQ_COM1 => {
            serial::handle_irq();
            pic::eoi(4);
        }
        YIELD_VECTOR => reschedule = Some(false),
        // IRQ 7 e 15 podem ser espúrias: só confirmamos se estiverem em serviço.
        39 if !pic::in_service(7) => {}
        47 if !pic::in_service(15) => pic::eoi(2),
        32..=47 => pic::eoi(vector - IRQ_BASE),
        _ => {}
    }
    finish(frame, reschedule)
}

/// Fim comum de toda entrada no kernel. Se a tarefa vai voltar ao anel 3,
/// entrega antes os sinais pendentes; depois troca de tarefa se pedido
/// (`Some(veio_do_timer)`) ou se a atual não pode mais executar.
pub fn finish(frame: &mut InterruptFrame, reschedule: Option<bool>) -> *mut InterruptFrame {
    if frame.from_user()
        && let Some(next) = signal::deliver(frame, None)
    {
        return next;
    }
    if task::current_is_dead() {
        return task::schedule(frame, false);
    }
    match reschedule {
        Some(from_timer) => task::schedule(frame, from_timer),
        None => frame,
    }
}

fn exception(frame: &mut InterruptFrame) -> *mut InterruptFrame {
    let fault_addr = cpu::read_cr2();
    // Falha de página em memória de usuário que ainda não foi alocada
    // (pilha, heap, mmap) ou que é cópia sob escrita: resolve e continua.
    if frame.vector == 14
        && fault_addr < USER_TOP
        && (frame.from_user() || task::current_is_user())
        && process::handle_page_fault(fault_addr, frame.error_code)
    {
        return frame;
    }

    let name = EXCEPTIONS[frame.vector as usize];
    if frame.from_user() {
        // Uma exceção no anel 3 vira um sinal para o processo.
        let signal = match frame.vector {
            0 | 16 | 19 => signal::SIGFPE,
            6 => signal::SIGILL,
            1 | 3 => signal::SIGTRAP,
            _ => signal::SIGSEGV,
        };
        let handled = task::with_current_process(|p| p.signals.action(signal).handler > signal::SIG_IGN);
        if signal != signal::SIGTRAP && handled != Some(true) {
            console::set_color(Color::LightRed);
            if frame.vector == 14 {
                println!(
                    "[pid {}] {}: {name} em {:#x} (rip={:#x})",
                    task::current_pid(),
                    signal::name(signal),
                    fault_addr,
                    frame.rip
                );
            } else {
                println!("[pid {}] {}: {name} (rip={:#x})", task::current_pid(), signal::name(signal), frame.rip);
            }
            console::reset_color();
        }
        task::force_signal_current(signal);
        return finish(frame, None);
    }

    if frame.vector == 3 {
        console::set_color(Color::Yellow);
        println!("[int3] ponto de parada em {:#x}", frame.rip);
        console::reset_color();
        return frame;
    }
    if frame.vector == 14 {
        panic!(
            "exceção: {} (#{}) acessando {:#x}\n  código={:#x} rip={:#x} rsp={:#x}",
            name, frame.vector, fault_addr, frame.error_code, frame.rip, frame.rsp
        );
    }
    panic!(
        "exceção: {} (#{})\n  código={:#x} rip={:#x} rsp={:#x} rflags={:#x}",
        name, frame.vector, frame.error_code, frame.rip, frame.rsp, frame.rflags
    );
}

/// O par de PICs 8259 encadeados.
mod pic {
    use super::*;

    const MASTER_CMD: u16 = 0x20;
    const MASTER_DATA: u16 = 0x21;
    const SLAVE_CMD: u16 = 0xA0;
    const SLAVE_DATA: u16 = 0xA1;

    pub fn init() {
        unsafe {
            // ICW1..ICW4: reprograma as IRQs para os vetores 32..47.
            outb(MASTER_CMD, 0x11);
            io_wait();
            outb(SLAVE_CMD, 0x11);
            io_wait();
            outb(MASTER_DATA, IRQ_BASE);
            io_wait();
            outb(SLAVE_DATA, IRQ_BASE + 8);
            io_wait();
            outb(MASTER_DATA, 4); // escravo na IRQ2
            io_wait();
            outb(SLAVE_DATA, 2);
            io_wait();
            outb(MASTER_DATA, 0x01); // modo 8086
            io_wait();
            outb(SLAVE_DATA, 0x01);
            io_wait();

            // Habilita só timer (0), teclado (1), cascata (2) e COM1 (4).
            outb(MASTER_DATA, !0b0001_0111);
            outb(SLAVE_DATA, 0xFF);
        }
    }

    pub fn eoi(irq: u8) {
        unsafe {
            if irq >= 8 {
                outb(SLAVE_CMD, 0x20);
            }
            outb(MASTER_CMD, 0x20);
        }
    }

    pub fn in_service(irq: u8) -> bool {
        unsafe {
            outb(MASTER_CMD, 0x0B);
            outb(SLAVE_CMD, 0x0B);
            let isr = (inb(SLAVE_CMD) as u16) << 8 | inb(MASTER_CMD) as u16;
            isr & (1 << irq) != 0
        }
    }
}
