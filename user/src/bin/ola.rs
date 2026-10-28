//! Olá, mundo do anel 3: mostra pid, sistema, argumentos e o nível de
//! privilégio em que está rodando.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use ferrugem_user::{entry, println, rt, sys};

entry!(main);

fn main() -> i32 {
    let cs: u16;
    unsafe { core::arch::asm!("mov {:x}, cs", out(reg) cs) };
    let system = sys::uname();
    let args: Vec<&str> = rt::args().collect();

    println!("Olá do espaço de usuário! Sou o processo {}.", sys::getpid());
    println!("Rodando em {} {} ({}).", system.field(0), system.field(2), system.field(4));
    println!("Nível de privilégio da CPU: anel {} (CS = {cs:#x}).", cs & 3);
    println!("Argumentos ({}): {args:?}", args.len());
    0
}
