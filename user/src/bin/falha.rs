//! Comete erros de propósito, para mostrar que o kernel encerra só o
//! processo culpado e o sistema continua de pé.

#![no_std]
#![no_main]

use core::arch::asm;
use core::hint::black_box;
use core::ptr;
use ferrugem_user::{entry, println, rt};

entry!(main);

fn main() -> i32 {
    let mode = rt::args().nth(1).unwrap_or("kernel");
    match mode {
        "kernel" => {
            println!("Lendo a memória do kernel em 0xffffffff80100000...");
            let value = unsafe { ptr::read_volatile(0xFFFF_FFFF_8010_0000 as *const u64) };
            println!("Li {value:#x}. Isso não deveria acontecer!");
        }
        "nulo" => {
            println!("Lendo o endereço 0...");
            let value = unsafe { ptr::read_volatile(black_box(ptr::null::<u64>())) };
            println!("Li {value:#x}. Isso não deveria acontecer!");
        }
        "codigo" => {
            println!("Escrevendo no meu próprio código (somente leitura)...");
            unsafe { ptr::write_volatile(main as *const () as *mut u8, 0xC3) };
            println!("Escrevi. Isso não deveria acontecer!");
        }
        "cli" => {
            println!("Executando `cli` (instrução privilegiada)...");
            unsafe { asm!("cli") };
            println!("Desliguei as interrupções. Isso não deveria acontecer!");
        }
        "div" => {
            println!("Dividindo por zero...");
            let zero = black_box(0u64);
            let result: u64;
            unsafe { asm!("div {}", in(reg) zero, inout("rax") 1u64 => result, inout("rdx") 0u64 => _) };
            println!("Resultado {result}. Isso não deveria acontecer!");
        }
        "pilha" => {
            println!("Recursão infinita: a pilha cresce sob demanda até o limite de 8 MiB...");
            println!("{}", recurse(0));
        }
        _ => {
            println!("uso: falha [kernel | nulo | codigo | cli | div | pilha]");
            return 2;
        }
    }
    1
}

#[allow(unconditional_recursion)]
fn recurse(depth: u64) -> u64 {
    let frame = black_box([depth; 64]);
    recurse(depth + 1) + frame[1]
}
