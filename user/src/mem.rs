//! As funções de memória que o compilador chama implicitamente. Sem libc,
//! alguém precisa fornecê-las; usamos as instruções de cadeia do x86 (que
//! o compilador não tem como transformar de volta em chamadas a memcpy).

use core::arch::asm;
use core::ffi::c_void;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    unsafe {
        asm!("rep movsb", inout("rdi") dest => _, inout("rsi") src => _, inout("rcx") n => _, options(nostack, preserves_flags))
    };
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    if (dest as usize) <= (src as usize) || (dest as usize) >= (src as usize + n) {
        return unsafe { memcpy(dest, src, n) };
    }
    // Sobreposição com o destino depois da origem: copia de trás para frente.
    unsafe {
        asm!(
            "std",
            "rep movsb",
            "cld",
            inout("rdi") dest.byte_add(n - 1) => _,
            inout("rsi") src.byte_add(n - 1) => _,
            inout("rcx") n => _,
            options(nostack),
        )
    };
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut c_void, value: i32, n: usize) -> *mut c_void {
    unsafe {
        asm!("rep stosb", inout("rdi") dest => _, inout("rcx") n => _, in("al") value as u8, options(nostack, preserves_flags))
    };
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    let (a, b) = (a as *const u8, b as *const u8);
    for i in 0..n {
        let (x, y) = unsafe { (a.add(i).read_volatile(), b.add(i).read_volatile()) };
        if x != y {
            return x as i32 - y as i32;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    unsafe { memcmp(a, b, n) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strlen(s: *const core::ffi::c_char) -> usize {
    let mut n = 0;
    while unsafe { s.add(n).read_volatile() } != 0 {
        n += 1;
    }
    n
}
