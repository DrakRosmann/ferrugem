//! Reiniciar e desligar.

use core::arch::asm;

use crate::cpu::{self, inb, outb, outw};

pub fn reboot() -> ! {
    crate::vfs::unmount_all(); // marca os discos como desmontados direito
    cpu::cli();
    unsafe {
        // Pede ao controlador do teclado (8042) para pulsar a linha de reset.
        while inb(0x64) & 0x02 != 0 {}
        outb(0x64, 0xFE);

        // Se não funcionou: IDT vazia + interrupção = falta tripla = reset.
        let empty = [0u8; 10];
        asm!("lidt [{}]", "int3", in(reg) &empty, options(noreturn));
    }
}

pub fn shutdown() -> ! {
    crate::vfs::unmount_all();
    cpu::cli();
    unsafe {
        outw(0x604, 0x2000); // ACPI do QEMU (máquinas pc e q35)
        outw(0xB004, 0x2000); // Bochs e QEMU antigos
        outw(0x4004, 0x3400); // VirtualBox
        outb(0xF4, 0x00); // isa-debug-exit, se presente
    }
    crate::println!("Não foi possível desligar; pode desligar a máquina com segurança.");
    cpu::halt_forever()
}
