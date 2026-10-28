//! Biblioteca mínima para programas de usuário do FerrugemOS.
//!
//! Faz o papel de uma libc bem pequena: ponto de entrada (`_start`),
//! chamadas de sistema, `print!`, alocador de memória sobre `brk` e as
//! funções de memória que o compilador espera (memcpy...). Como o kernel
//! segue a ABI do Linux, os programas também rodam no Linux sem mudança.

#![no_std]

extern crate alloc;

pub mod io;
mod heap;
mod mem;
pub mod rt;
pub mod sys;
