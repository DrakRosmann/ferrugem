//! reiniciar — reinicia a máquina.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let erro = sys::reiniciar();
    eprintln!("reiniciar: {}", erro);
    1
}
