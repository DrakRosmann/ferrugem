//! free — uso de memória.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let m = sys::memoria();
    println!("               total       usada       livre");
    println!(
        "RAM       {:>7} KiB {:>7} KiB {:>7} KiB",
        m.total_kib,
        m.total_kib - m.livre_kib,
        m.livre_kib
    );
    println!(
        "heap (k)  {:>7} KiB {:>7} KiB {:>7} KiB",
        m.heap_kernel_kib,
        m.heap_kernel_usado_kib,
        m.heap_kernel_kib - m.heap_kernel_usado_kib
    );
    0
}
