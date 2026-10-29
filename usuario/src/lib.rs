//! ulib — a "libc" dos programas do FerrugemOS.
//!
//! Um programa de usuário não tem `std` (ela precisaria de um port para este
//! sistema). Esta biblioteca dá o mínimo para escrever programas confortáveis:
//!
//! - `_start`: o ponto de entrada de verdade, que lê `argc`/`argv` da pilha;
//! - `sys`: as chamadas de sistema;
//! - um alocador (`Vec`, `String`, `Box`...) que pede memória com `brk`;
//! - `print!`, `println!`, `eprintln!` e leitura de linhas.
//!
//! Um programa fica assim:
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use ulib::*;
//! programa!(principal);
//! fn principal(args: &[&str]) -> i32 { println!("olá!"); 0 }
//! ```

#![no_std]

extern crate alloc;

pub mod grafico;
pub mod io;
pub mod janela;
#[path = "../../comum/lista_livre.rs"]
mod lista_livre;
mod memoria;
pub mod sha256;
pub mod sys;
pub mod usuarios;

pub use alloc::boxed::Box;
pub use alloc::format;
pub use alloc::string::{String, ToString};
pub use alloc::vec;
pub use alloc::vec::Vec;
pub use io::Leitor;

/// Declara a função principal do programa.
#[macro_export]
macro_rules! programa {
    ($principal:path) => {
        #[unsafe(no_mangle)]
        fn __ulib_principal(args: &[&str]) -> i32 {
            $principal(args)
        }
    };
}

unsafe extern "Rust" {
    fn __ulib_principal(args: &[&str]) -> i32;
}

static mut AMBIENTE: Vec<&'static str> = Vec::new();

/// As variáveis de ambiente que o programa recebeu ("NOME=valor").
pub fn ambiente() -> &'static [&'static str] {
    unsafe { &*(&raw const AMBIENTE) }
}

/// O valor de uma variável de ambiente, como o `getenv` do C.
pub fn variavel(nome: &str) -> Option<&'static str> {
    ambiente().iter().find_map(|v| v.strip_prefix(nome).and_then(|r| r.strip_prefix('=')))
}

/// Onde o kernel salta. A pilha está assim:
/// `argc | argv[0] .. argv[n-1] | 0 | envp[0] .. | 0 | auxv...`
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp", // 1º argumento: ponteiro para argc
        "and rsp, -16",
        "call {}",
        "ud2",
        sym iniciar,
    )
}

unsafe extern "C" fn iniciar(pilha: *const u64) -> ! {
    let argc = unsafe { *pilha } as usize;
    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        let ptr = unsafe { *pilha.add(1 + i) } as *const u8;
        args.push(unsafe { string_c(ptr) });
    }
    let mut ambiente = Vec::new();
    let mut i = argc + 2;
    loop {
        let ptr = unsafe { *pilha.add(i) } as *const u8;
        if ptr.is_null() {
            break;
        }
        ambiente.push(unsafe { string_c(ptr) });
        i += 1;
    }
    unsafe { *(&raw mut AMBIENTE) = ambiente };
    let codigo = unsafe { __ulib_principal(&args) };
    sys::sair(codigo)
}

/// # Safety
/// `ptr` aponta para uma string terminada em zero que vive para sempre.
unsafe fn string_c(ptr: *const u8) -> &'static str {
    let mut n = 0;
    while unsafe { *ptr.add(n) } != 0 {
        n += 1;
    }
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, n) }).unwrap_or("?")
}

#[panic_handler]
fn panico(info: &core::panic::PanicInfo) -> ! {
    eprintln!("pânico: {}", info.message());
    sys::sair(101)
}
