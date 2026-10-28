//! Exercita a memória de um processo: heap via `brk`, uma região de `mmap`
//! e uma pilha funda. As páginas só existem quando tocadas pela primeira
//! vez (o kernel as cria no tratador de falha de página).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::hint::black_box;
use ferrugem_user::{entry, println, rt, sys};

entry!(main);

const MIB: usize = 1 << 20;

fn main() -> i32 {
    let pause_ms: u64 = rt::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(0);

    let brk_start = sys::brk(0);
    let numbers: Vec<u64> = (0..(4 * MIB / 8) as u64).collect();
    let sum: u64 = numbers.iter().sum();
    let expected = numbers.len() as u64 * (numbers.len() as u64 - 1) / 2;
    println!(
        "heap: vetor de 4 MiB somou {sum} ({}); brk cresceu {} KiB",
        if sum == expected { "ok" } else { "ERRADO" },
        (sys::brk(0) - brk_start) / 1024
    );

    let len = 16 * MIB;
    let region = match sys::mmap_anonymous(len) {
        Ok(ptr) => ptr,
        Err(errno) => {
            println!("mmap falhou: errno {errno}");
            return 1;
        }
    };
    // Toca uma página a cada 64 KiB: só essas 256 páginas viram memória real.
    let mut ok = true;
    for offset in (0..len).step_by(64 * 1024) {
        unsafe { region.add(offset).write_volatile((offset / 4096) as u8) };
    }
    for offset in (0..len).step_by(64 * 1024) {
        ok &= unsafe { region.add(offset).read_volatile() } == (offset / 4096) as u8;
    }
    println!("mmap: região de 16 MiB em {region:p}, 256 páginas tocadas: {}", if ok { "ok" } else { "ERRADO" });

    let depth = 2000;
    println!("pilha: recursão de {depth} níveis (~1 MiB) devolveu {}", deep(depth));

    if pause_ms > 0 {
        println!("pausando {pause_ms} ms (veja a coluna MEMÓRIA no `ps`)...");
        let _ = sys::sleep_ms(pause_ms);
    }
    sys::munmap(region, len);
    if ok && sum == expected { 0 } else { 1 }
}

/// Cada nível usa ~512 bytes de pilha.
fn deep(level: u64) -> u64 {
    let frame = black_box([level; 64]);
    if level == 0 { 0 } else { deep(level - 1) + frame[0] - level + 1 }
}
