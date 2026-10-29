//! FerrugemOS — um kernel x86_64 mínimo em Rust.
//!
//! `#![no_std]`: não existe biblioteca padrão, porque ela depende de um SO
//! (arquivos, threads, alocação...). Só temos `core`: tipos, traits,
//! formatação, iteradores, atômicos. Todo o resto, nós escrevemos.

#![no_std]

extern crate alloc;

mod arquivo;
mod ata;
mod elf;
mod erro;
mod ext2;
mod gdt;
mod heap;
mod initrd;
mod interrupcoes;
#[path = "../comum/lista_livre.rs"]
mod lista_livre;
mod multiboot;
mod paginacao;
mod pic;
mod pit;
mod porta;
mod processo;
mod quadros;
mod rtc;
mod serial;
mod sinal;
mod spin;
mod syscall;
mod teclado;
mod tty;
mod vfs;
mod vga;

use core::arch::asm;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use vga::Cor;

// -----------------------------------------------------------------------------
// print! e println! — escrevem na tela VGA e na porta serial ao mesmo tempo
// -----------------------------------------------------------------------------

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // Sem interrupções enquanto seguramos os locks: se uma interrupção chegasse
    // agora e também quisesse imprimir, ela esperaria por nós para sempre.
    interrupcoes::sem_interrupcoes(|| {
        let _ = vga::TELA.lock().write_fmt(args);
        let _ = serial::SERIAL.lock().write_fmt(args);
    });
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

// -----------------------------------------------------------------------------
// Ponto de entrada (chamado por boot.asm, já em modo longo de 64 bits)
// -----------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn kmain(magico: u32, endereco_info: u32) -> ! {
    serial::init();
    vga::limpar();

    vga::definir_cor(Cor::CianoClaro, Cor::Preto);
    println!("╔══════════════════════════════════════════════╗");
    println!("║   FerrugemOS 0.4  ·  um kernel feito em Rust ║");
    println!("╚══════════════════════════════════════════════╝");
    vga::definir_cor(Cor::CinzaClaro, Cor::Preto);

    if magico != multiboot::MAGICO {
        panic!("número mágico do Multiboot inválido: {:#x}", magico);
    }
    let info = unsafe { multiboot::Info::de_endereco(endereco_info) };

    gdt::init();
    interrupcoes::init();
    syscall::init();
    ok(format_args!(
        "carregado por {}; modo longo, GDT/TSS, IDT e syscall configurados",
        info.nome_bootloader().unwrap_or("?")
    ));

    let livres = quadros::init(&info);
    heap::init();
    paginacao::init();
    ok(format_args!(
        "memória: {} MiB livres em quadros de 4 KiB; heap do kernel de {} MiB",
        livres * 4 / 1024,
        heap::TAMANHO / 1024 / 1024
    ));

    rtc::init();
    vfs::init();
    let Some(modulo) = info.modulos().next() else {
        panic!("nenhum initrd: rode com `make run` (que passa -initrd ao QEMU)");
    };
    let dados = unsafe {
        core::slice::from_raw_parts(modulo.inicio as *const u8, (modulo.fim - modulo.inicio) as usize)
    };
    ok(format_args!("initrd: {} arquivos carregados em / (sistema de arquivos em memória)", initrd::carregar(dados)));

    match ata::init() {
        None => println!("  [ -- ] nenhum disco ATA: /disco não vai existir"),
        Some(setores) => match ext2::Ext2::montar() {
            Ok(fs) => {
                let _ = vfs::montar("/disco", fs);
                ok(format_args!("disco ATA de {} MiB com ext2 montado em /disco", setores / 2048));
            }
            Err(motivo) => println!("  [ -- ] disco ATA de {} MiB, mas {}", setores / 2048, motivo),
        },
    }

    pic::init();
    pit::init();
    teclado::init();
    ok(format_args!("PIC, timer a {} Hz, teclado ABNT2 e serial", pit::FREQUENCIA));

    processo::init();
    if let Err(erro) = processo::criar_init("/bin/init") {
        panic!("não consegui carregar /bin/init (erro {})", erro);
    }
    ok("processo 1 (/bin/init) criado; passando o controle ao escalonador");
    println!();

    // Daqui em diante, este fluxo é o processo 0: o ocioso.
    processo::ocioso()
}

fn ok(mensagem: impl fmt::Display) {
    vga::definir_cor(Cor::VerdeClaro, Cor::Preto);
    print!("  [ OK ] ");
    vga::definir_cor(Cor::CinzaClaro, Cor::Preto);
    println!("{}", mensagem);
}

/// Desliga as interrupções e deixa a CPU dormindo para sempre.
/// (`hlt` gasta bem menos energia que um `loop {}` vazio.)
fn parar() -> ! {
    interrupcoes::desabilitar();
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

// -----------------------------------------------------------------------------
// Pânico: sem SO para mostrar um erro, nós mesmos mostramos e paramos.
// -----------------------------------------------------------------------------

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    match info.location() {
        Some(local) => erro_fatal(format_args!(
            "{}\n  em {}:{}",
            info.message(),
            local.file(),
            local.line()
        )),
        None => erro_fatal(format_args!("{}", info.message())),
    }
}

/// Mostra o erro em vermelho e para. Usada pelo pânico e pelas exceções da CPU.
pub fn erro_fatal(mensagem: fmt::Arguments) -> ! {
    interrupcoes::desabilitar();
    // Se o erro aconteceu no meio de um print!, os locks da tela e da serial
    // ficaram presos com quem quebrou. Ninguém mais vai rodar: soltamos à força.
    unsafe {
        vga::TELA.forcar_destravar();
        serial::SERIAL.forcar_destravar();
    }
    vga::definir_cor(Cor::Branco, Cor::Vermelho);
    println!();
    println!("KERNEL PANIC: {}", mensagem);
    parar()
}

// -----------------------------------------------------------------------------
// memcpy/memset/memmove/memcmp
//
// O compilador gera chamadas a essas funções por conta própria (ao copiar
// structs, zerar arrays...). No alvo `x86_64-unknown-none` o Rust já as
// fornece. Este bloco só existe para compilar com o alvo Linux do host,
// caso o alvo bare metal não esteja instalado.
// -----------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod mem {
    use core::arch::asm;

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        unsafe {
            asm!("rep movsb", inout("rdi") dest => _, inout("rsi") src => _, inout("rcx") n => _,
                 options(nostack, preserves_flags));
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        if (dest as usize) <= (src as usize) || (dest as usize) >= (src as usize + n) {
            return unsafe { memcpy(dest, src, n) };
        }
        // Regiões sobrepostas com destino depois da origem: copia de trás para frente.
        unsafe {
            asm!("std", "rep movsb", "cld",
                 inout("rdi") dest.add(n - 1) => _, inout("rsi") src.add(n - 1) => _,
                 inout("rcx") n => _, options(nostack));
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memset(dest: *mut u8, c: i32, n: usize) -> *mut u8 {
        unsafe {
            asm!("rep stosb", inout("rdi") dest => _, inout("rcx") n => _, in("al") c as u8,
                 options(nostack, preserves_flags));
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        let mut i = 0;
        while i < n {
            let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
            if x != y {
                return x as i32 - y as i32;
            }
            i += 1;
        }
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        unsafe { memcmp(a, b, n) }
    }
}
