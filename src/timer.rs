//! Temporizador programável 8253/8254 (PIT), canal 0.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::cpu::outb;

pub const HZ: u64 = 100;
const PIT_FREQUENCY: u64 = 1_193_182;

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let divisor = (PIT_FREQUENCY / HZ) as u16;
    unsafe {
        outb(0x43, 0x36); // canal 0, byte baixo/alto, modo 3 (onda quadrada)
        outb(0x40, divisor as u8);
        outb(0x40, (divisor >> 8) as u8);
    }
}

pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    ticks() * 1000 / HZ
}

pub fn ms_to_ticks(ms: u64) -> u64 {
    ms.div_ceil(1000 / HZ)
}
