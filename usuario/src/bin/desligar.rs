//! desligar — desliga a máquina (no QEMU).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let erro = sys::desligar();
    eprintln!("desligar: {} (sem ACPI, só funciona no QEMU/Bochs)", erro);
    1
}
