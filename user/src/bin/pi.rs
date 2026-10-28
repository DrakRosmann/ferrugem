//! Calcula π pela série de Leibniz em ponto flutuante (SSE). Rode várias
//! cópias ao mesmo tempo (`pi &`): se o kernel não salvasse os registradores
//! da FPU na troca de tarefa, elas corromperiam as contas umas das outras.

#![no_std]
#![no_main]

use ferrugem_user::{entry, println, rt, sys};

entry!(main);

fn main() -> i32 {
    let terms: u64 = rt::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20_000_000);
    let start = sys::clock_ms();
    let mut sum = 0.0f64;
    let mut sign = 1.0f64;
    for k in 0..terms {
        sum += sign / (2 * k + 1) as f64;
        sign = -sign;
    }
    let pi = 4.0 * sum;
    let error = (pi - core::f64::consts::PI).abs();
    // A série alternada tem erro ≈ 1/termos; muito acima disso é corrupção.
    let ok = error < 2.0 / terms as f64;
    println!(
        "[pid {}] π ≈ {pi:.12} com {terms} termos, erro {error:.1e} em {} ms: {}",
        sys::getpid(),
        sys::clock_ms() - start,
        if ok { "correto" } else { "ERRADO" }
    );
    if ok { 0 } else { 1 }
}
