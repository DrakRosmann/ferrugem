//! PIT 8253/8254: o timer programável do PC.
//!
//! Ele tem um oscilador de 1.193.182 Hz e um contador de 16 bits. Programamos
//! o canal 0 com um divisor: a cada `divisor` pulsos ele dispara a IRQ 0.
//! Esse "tique" periódico é o relógio do kernel e, mais adiante, o que vai
//! permitir ao escalonador tirar a CPU de uma tarefa e dar para outra.

use crate::porta::outb;
use core::sync::atomic::{AtomicU64, Ordering};

const FREQUENCIA_BASE: u32 = 1_193_182;
pub const FREQUENCIA: u32 = 100; // tiques por segundo

static TIQUES: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let divisor = (FREQUENCIA_BASE / FREQUENCIA) as u16;
    unsafe {
        // Canal 0 | acesso: byte baixo e depois alto | modo 3 (onda quadrada)
        outb(0x43, 0b00_11_011_0);
        outb(0x40, divisor as u8);
        outb(0x40, (divisor >> 8) as u8);
    }
}

/// Chamada pela IRQ 0.
pub fn tique() {
    TIQUES.fetch_add(1, Ordering::Relaxed);
}

pub fn tiques() -> u64 {
    TIQUES.load(Ordering::Relaxed)
}

pub fn milissegundos() -> u64 {
    tiques() * 1000 / FREQUENCIA as u64
}
