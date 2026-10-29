//! uptime — há quanto tempo o sistema está ligado.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let ms = sys::tempo_ms();
    let s = ms / 1000;
    println!("ligado há {}h {:02}m {:02}.{:02}s", s / 3600, s / 60 % 60, s % 60, ms % 1000 / 10);
    0
}
