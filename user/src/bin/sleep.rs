//! sleep segundos: espera (aceita frações, como 0.5).

#![no_std]
#![no_main]

use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn main() -> i32 {
    let Some(arg) = rt::args().nth(1) else {
        eprintln!("uso: sleep segundos");
        return 2;
    };
    let (whole, fraction) = arg.split_once('.').unwrap_or((arg, ""));
    let (Ok(seconds), Ok(millis)) = (
        whole.parse::<u64>(),
        if fraction.is_empty() { Ok(0) } else { alloc::format!("{fraction:0<3}")[..3].parse::<u64>() },
    ) else {
        eprintln!("sleep: tempo inválido: {arg}");
        return 2;
    };
    match sys::sleep_ms(seconds * 1000 + millis) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

extern crate alloc;
