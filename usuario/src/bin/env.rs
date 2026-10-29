//! env — mostra as variáveis de ambiente.

#![no_std]
#![no_main]

use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    for v in ambiente() {
        println!("{}", v);
    }
    0
}
