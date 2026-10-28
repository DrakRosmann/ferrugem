//! FerrugemOS: um sistema operacional x86_64 feito do zero em Rust.

#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
mod console;
mod block;
mod cpu;
mod elf;
mod ext2;
mod file;
mod gdt;
mod heap;
mod input;
mod interrupts;
mod keyboard;
mod memfs;
mod memory;
mod multiboot;
mod paging;
mod pipe;
mod power;
mod process;
mod rtc;
mod serial;
mod shell;
mod signal;
mod sync;
mod syscall;
mod task;
mod timer;
mod tty;
mod vfs;
mod vga;

use core::arch::global_asm;
use core::panic::PanicInfo;

use alloc::format;
use alloc::sync::Arc;
use block::BlockDevice;
use vga::Color;

global_asm!(include_str!("boot.s"), options(att_syntax, raw));

/// Chamado por boot.s, já em modo longo e executando na metade alta.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(_magic: u32, multiboot_info: u32) -> ! {
    serial::init();
    vga::init();

    console::set_color(Color::LightRed);
    println!("  ┌──────────────────────────────────┐");
    println!("  │  F E R R U G E M   O S   {:<8}│", env!("CARGO_PKG_VERSION"));
    println!("  └──────────────────────────────────┘");
    console::reset_color();
    println!("  Um sistema operacional escrito do zero em Rust.\n");

    // A info do Multiboot é copiada antes de o alocador de quadros
    // poder reutilizar a memória onde ela está.
    let boot = unsafe { multiboot::BootInfo::parse(multiboot_info) };

    gdt::init();
    ok("GDT e TSS carregados");
    interrupts::init();
    timer::init();
    rtc::init();
    serial::enable_rx_interrupt();
    ok("IDT, PIC e timer (100 Hz) configurados");
    paging::init();
    ok("kernel na metade alta; metade baixa livre para processos");
    memory::init(&boot);
    let (total, free) = memory::stats();
    ok(format_args!("{} MiB de RAM, {} MiB livres", total >> 20, free >> 20));
    heap::init();
    ok(format_args!("heap de {} pronto", shell::format_size(heap::stats().0 as u64)));
    syscall::init();
    ok("chamadas de sistema (syscall, ABI do Linux) habilitadas");
    mount_filesystems();
    task::init();
    task::spawn("relógio", status_bar);
    ok("escalonador preemptivo iniciado");

    cpu::sti();
    println!("\nDigite 'ajuda' para ver os comandos.\n");
    shell::run(boot)
}

/// Monta a raiz (o ext2 do disco ATA, se houver; senão, a embutida) e,
/// por cima, /dev e /tmp na memória.
fn mount_filesystems() {
    let mut on_disk = false;
    if let Some(disk) = block::AtaDisk::probe(false) {
        let description = format!("{}, {}", disk.name(), shell::format_size(disk.sectors() * 512));
        let disk: Arc<dyn BlockDevice> = Arc::new(disk);
        match ext2::Ext2::mount(disk, 1) {
            Ok(root) => {
                vfs::mount("/", root, "ext2", "/dev/hda");
                ok(format_args!("disco ATA ({description}): ext2 montado em /"));
                on_disk = true;
            }
            Err(reason) => warn(format_args!("disco ATA ({description}): {reason}; usando a raiz em memória")),
        }
    }
    if !on_disk {
        vfs::mount("/", memfs::builtin_root(), "memfs", "embutido");
        ok("raiz em memória com os programas embutidos (sem disco: nada é salvo)");
    }
    vfs::mount("/dev", memfs::devfs(), "devfs", "devfs");
    vfs::mount("/tmp", memfs::new(), "memfs", "tmpfs");
    ok("/dev e /tmp montados na memória");
}

fn warn(message: impl core::fmt::Display) {
    print!("  [");
    console::set_color(Color::Yellow);
    print!(" !! ");
    console::reset_color();
    println!("] {message}");
}

fn ok(message: impl core::fmt::Display) {
    print!("  [");
    console::set_color(Color::LightGreen);
    print!(" ok ");
    console::reset_color();
    println!("] {message}");
}

/// Tarefa que mantém a barra de status (linha 0) atualizada.
fn status_bar() {
    loop {
        let (heap_size, heap_used) = heap::stats();
        let text = format!(
            " FerrugemOS │ {} │ {} tarefas │ heap {}/{} ",
            shell::format_duration(timer::uptime_ms()),
            task::count(),
            shell::format_size(heap_used as u64),
            shell::format_size(heap_size as u64),
        );
        vga::status_text(0, &format!("{text:<62}"), vga::STATUS_ATTR);
        task::sleep_ms(500);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    cpu::cli();
    unsafe {
        vga::WRITER.force_unlock();
        serial::SERIAL.force_unlock();
    }
    console::set_color(Color::LightRed);
    println!("\n*** PÂNICO DO KERNEL ***");
    if let Some(location) = info.location() {
        println!("em {}:{}", location.file(), location.line());
    }
    println!("{}", info.message());
    console::reset_color();
    println!("Sistema parado.");
    cpu::halt_forever()
}
