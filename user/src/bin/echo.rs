//! echo [-n] [texto...]: imprime os argumentos.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use ferrugem_user::{entry, rt, sys};

entry!(main);

fn main() -> i32 {
    let mut args: alloc::vec::Vec<&str> = rt::args().skip(1).collect();
    let newline = args.first() != Some(&"-n");
    if !newline {
        args.remove(0);
    }
    let mut text = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        text.push_str(arg);
    }
    if newline {
        text.push('\n');
    }
    if sys::write_all(1, text.as_bytes()).is_err() { 1 } else { 0 }
}
