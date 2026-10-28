//! Início e fim do programa: `_start`, argumentos e pânico.

use crate::sys;

static mut ARGC: usize = 0;
static mut ARGV: *const *const u8 = core::ptr::null();
static mut ENVP: *const *const u8 = core::ptr::null();

/// Define o `_start` do programa, que chama `main` e sai com o código que
/// ela devolver. Uso: `ferrugem_user::entry!(main);`
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "C" fn _start() -> ! {
            // O kernel entrega rsp apontando para argc (ABI System V).
            core::arch::naked_asm!(
                "xor ebp, ebp",
                "mov rdi, rsp",
                "lea rsi, [rip + {main}]",
                "and rsp, -16",
                "call {start}",
                "ud2",
                main = sym $main,
                start = sym $crate::rt::start,
            )
        }
    };
}

#[doc(hidden)]
pub extern "C" fn start(stack: *const u64, main: usize) -> ! {
    unsafe {
        ARGC = *stack as usize;
        ARGV = stack.add(1) as *const *const u8;
        ENVP = ARGV.add(ARGC + 1); // o argv termina com NULL; o ambiente vem logo depois
    }
    let main: fn() -> i32 = unsafe { core::mem::transmute(main) };
    sys::exit(main())
}

unsafe fn c_str(ptr: *const u8) -> &'static str {
    unsafe {
        let len = (0..).take_while(|&j| *ptr.add(j) != 0).count();
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("?")
    }
}

/// Os argumentos da linha de comando (o primeiro é o nome do programa).
pub fn args() -> impl Iterator<Item = &'static str> {
    let (argc, argv) = unsafe { (ARGC, ARGV) };
    (0..argc).map(move |i| unsafe { c_str(*argv.add(i)) })
}

/// O ambiente, como strings "NOME=valor".
pub fn env() -> impl Iterator<Item = &'static str> {
    let envp = unsafe { ENVP };
    (0..).map(move |i| unsafe { *envp.add(i) }).take_while(|p| !p.is_null()).map(|p| unsafe { c_str(p) })
}

pub fn getenv(name: &str) -> Option<&'static str> {
    env().find_map(|entry| entry.strip_prefix(name)?.strip_prefix('='))
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    crate::eprintln!("pânico: {}", info.message());
    sys::exit(101)
}

// O `alloc` pré-compilado referencia o mecanismo de desenrolar a pilha em
// pânicos. Com panic=abort ele nunca é usado, mas os símbolos precisam
// existir para a ligação.

#[unsafe(no_mangle)]
extern "C" fn _Unwind_Resume() -> ! {
    sys::exit(134)
}

#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() {}
