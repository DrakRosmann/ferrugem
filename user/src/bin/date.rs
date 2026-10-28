//! date: a data e a hora atuais (UTC), lidas do relógio do sistema.

#![no_std]
#![no_main]

use ferrugem_user::{entry, println, sys};

entry!(main);

fn main() -> i32 {
    let (year, month, day, hour, minute, second) = sys::civil_time(sys::time());
    println!("{year}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC");
    0
}
