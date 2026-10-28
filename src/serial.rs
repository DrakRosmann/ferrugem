//! Porta serial COM1 (UART 16550): saída de log e entrada alternativa ao teclado.

use core::fmt;

use crate::cpu::{inb, outb};
use crate::input;
use crate::sync::SpinLock;

const COM1: u16 = 0x3F8;

pub struct Serial {
    present: bool,
}

pub static SERIAL: SpinLock<Serial> = SpinLock::new(Serial { present: false });

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // sem interrupções por enquanto
        outb(COM1 + 3, 0x80); // DLAB ligado para definir o divisor
        outb(COM1, 0x01); // 115200 baud
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8 bits, sem paridade, 1 bit de parada
        outb(COM1 + 2, 0xC7); // FIFO ligado, limpo, gatilho de 14 bytes
        outb(COM1 + 4, 0x0B); // DTR, RTS e OUT2 (necessário para IRQ)
    }
    // Uma porta inexistente lê 0xFF no registrador de status de linha.
    let present = unsafe { inb(COM1 + 5) } != 0xFF;
    SERIAL.lock().present = present;
}

/// Liga a interrupção de "dado recebido" (IRQ4).
pub fn enable_rx_interrupt() {
    if SERIAL.lock().present {
        unsafe { outb(COM1 + 1, 0x01) }
    }
}

pub fn handle_irq() {
    while unsafe { inb(COM1 + 5) } & 1 != 0 {
        let byte = unsafe { inb(COM1) };
        // Terminais mandam CR no Enter e DEL no backspace.
        let byte = match byte {
            b'\r' => b'\n',
            0x7F => 0x08,
            b => b,
        };
        input::push(byte);
    }
}

impl Serial {
    fn write_byte(&mut self, byte: u8) {
        if !self.present {
            return;
        }
        while unsafe { inb(COM1 + 5) } & 0x20 == 0 {
            core::hint::spin_loop();
        }
        unsafe { outb(COM1, byte) }
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}
