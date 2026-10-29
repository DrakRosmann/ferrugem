//! Acesso a portas de E/S do x86 (instruções `in` e `out`).
//!
//! Dispositivos antigos do PC (porta serial, teclado PS/2, PIC, timer PIT...)
//! não ficam mapeados na memória: são acessados por um espaço de endereços
//! separado de 16 bits, o espaço de "portas".

use core::arch::asm;

/// Escreve um byte numa porta.
///
/// # Safety
/// Escrever na porta errada pode reconfigurar hardware de forma inesperada.
pub unsafe fn outb(porta: u16, valor: u8) {
    unsafe {
        asm!("out dx, al", in("dx") porta, in("al") valor, options(nomem, nostack, preserves_flags));
    }
}

/// Escreve 16 bits numa porta.
///
/// # Safety
/// Idem `outb`.
pub unsafe fn outw(porta: u16, valor: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") porta, in("ax") valor, options(nomem, nostack, preserves_flags));
    }
}

/// Lê 16 bits de uma porta.
///
/// # Safety
/// Idem `inb`.
pub unsafe fn inw(porta: u16) -> u16 {
    let valor: u16;
    unsafe {
        asm!("in ax, dx", out("ax") valor, in("dx") porta, options(nomem, nostack, preserves_flags));
    }
    valor
}

/// Lê um byte de uma porta.
///
/// # Safety
/// Ler algumas portas tem efeitos colaterais no dispositivo.
pub unsafe fn inb(porta: u16) -> u8 {
    let valor: u8;
    unsafe {
        asm!("in al, dx", out("al") valor, in("dx") porta, options(nomem, nostack, preserves_flags));
    }
    valor
}

/// Escreve 32 bits numa porta.
///
/// # Safety
/// Idem `outb`.
pub unsafe fn outl(porta: u16, valor: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") porta, in("eax") valor, options(nomem, nostack, preserves_flags));
    }
}

/// Lê 32 bits de uma porta.
///
/// # Safety
/// Idem `inb`.
pub unsafe fn inl(porta: u16) -> u32 {
    let valor: u32;
    unsafe {
        asm!("in eax, dx", out("eax") valor, in("dx") porta, options(nomem, nostack, preserves_flags));
    }
    valor
}
