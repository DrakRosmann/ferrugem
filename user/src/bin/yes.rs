//! yes [texto]: repete o texto (ou "y") para sempre. Termina quando quem
//! lê fecha o pipe (o kernel envia SIGPIPE).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use ferrugem_user::{entry, rt, sys};

entry!(main);

fn main() -> i32 {
    let words: Vec<&str> = rt::args().skip(1).collect();
    let mut line = if words.is_empty() { String::from("y") } else { words.join(" ") };
    line.push('\n');
    // Um bloco com várias cópias, para não fazer uma syscall por linha.
    let block = line.repeat((4096 / line.len()).max(1));
    while sys::write_all(1, block.as_bytes()).is_ok() {}
    1
}
