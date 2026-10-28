//! Instruções de baixo nível: portas de E/S, controle de interrupções, CPUID.

use core::arch::asm;

#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags)) }
}

#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe { asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags)) }
    value
}

#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags)) }
}

/// Pequena espera usada entre comandos ao PIC antigo.
#[inline]
pub unsafe fn io_wait() {
    unsafe { outb(0x80, 0) }
}

#[inline]
pub fn cli() {
    unsafe { asm!("cli", options(nomem, nostack)) }
}

#[inline]
pub fn sti() {
    unsafe { asm!("sti", options(nomem, nostack)) }
}

#[inline]
pub fn hlt() {
    unsafe { asm!("hlt", options(nomem, nostack)) }
}

/// Habilita interrupções e para até a próxima, sem janela de corrida:
/// o `sti` só tem efeito depois da instrução seguinte.
#[inline]
pub fn sti_hlt() {
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) }
}

pub fn interrupts_enabled() -> bool {
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags)) }
    flags & (1 << 9) != 0
}

pub fn read_cr2() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) }
    value
}

pub fn halt_forever() -> ! {
    loop {
        cli();
        hlt();
    }
}

pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

pub fn cpuid(leaf: u32) -> CpuidResult {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    // O LLVM reserva rbx, então ele é preservado manualmente.
    unsafe {
        asm!(
            "mov {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") 0 => ecx,
            out("edx") edx,
            options(nomem, nostack, preserves_flags),
        )
    }
    CpuidResult { eax, ebx, ecx, edx }
}

pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags)) }
    (high as u64) << 32 | low as u64
}

pub unsafe fn wrmsr(msr: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack, preserves_flags))
    }
}

pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) }
    value
}

/// Troca o espaço de endereçamento (e descarta o TLB não global).
pub unsafe fn write_cr3(value: u64) {
    unsafe { asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags)) }
}

pub fn invlpg(addr: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags)) }
}

pub fn rdtsc() -> u64 {
    let (low, high): (u32, u32);
    unsafe { asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags)) }
    (high as u64) << 32 | low as u64
}

/// Estado da FPU/SSE de uma tarefa, no formato do FXSAVE.
#[repr(C, align(16))]
#[derive(Clone)]
pub struct FpuState([u8; 512]);

impl FpuState {
    /// Estado limpo: x87 reiniciada e MXCSR padrão (todas as exceções mascaradas).
    pub fn clean() -> Self {
        let mut state = FpuState([0; 512]);
        state.0[0] = 0x7F; // FCW = 0x037F
        state.0[1] = 0x03;
        state.0[24..28].copy_from_slice(&0x1F80u32.to_le_bytes()); // MXCSR
        state
    }

    pub fn from_bytes(bytes: &[u8; 512]) -> Self {
        let mut state = FpuState(*bytes);
        // MXCSR com bits reservados ligados faria o fxrstor falhar (#GP).
        let mxcsr = u32::from_le_bytes(state.0[24..28].try_into().unwrap()) & 0xFFBF;
        state.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
        state
    }

    pub fn as_bytes(&self) -> &[u8; 512] {
        &self.0
    }

    pub fn save(&mut self) {
        unsafe { asm!("fxsave64 [{}]", in(reg) self.0.as_mut_ptr(), options(nostack, preserves_flags)) }
    }

    pub fn restore(&self) {
        unsafe { asm!("fxrstor64 [{}]", in(reg) self.0.as_ptr(), options(nostack, preserves_flags)) }
    }
}
