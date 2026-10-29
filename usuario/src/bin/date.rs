//! date — data e hora atuais (UTC, lidas do relógio CMOS na partida).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    println!("{} UTC", sys::formatar_data(sys::hora_atual()));
    0
}
