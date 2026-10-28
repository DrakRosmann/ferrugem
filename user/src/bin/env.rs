//! env: mostra as variáveis de ambiente.

#![no_std]
#![no_main]

use ferrugem_user::{entry, println, rt};

entry!(main);

fn main() -> i32 {
    for entry in rt::env() {
        println!("{entry}");
    }
    0
}
