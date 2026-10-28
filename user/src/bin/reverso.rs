//! Lê linhas do teclado (pela syscall `read`) e as devolve invertidas e em
//! maiúsculas. Termina com "sair" ou Ctrl+D.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use ferrugem_user::{entry, io, print, println};

entry!(main);

fn main() -> i32 {
    println!("Digite algo (\"sair\" ou Ctrl+D para terminar):");
    let mut buf = [0u8; 256];
    loop {
        print!("eco> ");
        let Some(line) = io::read_line(&mut buf) else { break };
        if line == "sair" {
            break;
        }
        let reversed: String = line.chars().rev().collect();
        println!("{}", reversed.to_uppercase());
    }
    println!("tchau!");
    0
}
