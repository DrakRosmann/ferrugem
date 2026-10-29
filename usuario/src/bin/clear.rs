//! clear — limpa a tela (sequência ANSI que a VGA e o terminal entendem).

#![no_std]
#![no_main]

use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    print!("\x1b[2J\x1b[H");
    0
}
