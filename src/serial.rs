//! Driver da porta serial COM1 (chip UART 16550).
//!
//! A serial é a melhor amiga de quem escreve kernel: o QEMU mostra tudo o que
//! sai por ela no seu terminal (`-serial stdio`), então dá para ver logs mesmo
//! quando a tela trava, e dá para copiar e colar a saída.
//!
//! Ela também é uma entrada: o que você digita no terminal do QEMU chega como
//! bytes na COM1 (IRQ 4) e vai para o terminal, como se viesse do teclado.
//! É assim que o shell funciona com `make run-nox`, sem teclado PS/2.

use crate::porta::{inb, outb};
use crate::spin::Mutex;
use crate::tty;
use core::fmt;

const COM1: u16 = 0x3F8;

pub struct Serial {
    base: u16,
}

impl Serial {
    const fn new(base: u16) -> Self {
        Self { base }
    }

    fn init(&mut self) {
        let b = self.base;
        unsafe {
            outb(b + 1, 0x00); // desliga interrupções da UART
            outb(b + 3, 0x80); // DLAB = 1: os próximos dois registradores são o divisor
            outb(b, 0x03);     // divisor = 3 -> 115200 / 3 = 38400 baud
            outb(b + 1, 0x00);
            outb(b + 3, 0x03); // 8 bits, sem paridade, 1 stop bit (8N1), DLAB = 0
            outb(b + 2, 0xC7); // liga FIFO, limpa, gatilho de 14 bytes
            outb(b + 4, 0x0B); // DTR + RTS + OUT2 (OUT2 liga a linha de IRQ)
            outb(b + 1, 0x01); // interrupção quando chegar um byte
        }
    }

    fn pode_enviar(&self) -> bool {
        // Bit 5 do Line Status Register: buffer de transmissão vazio
        unsafe { inb(self.base + 5) & 0x20 != 0 }
    }

    pub fn escrever_byte(&mut self, byte: u8) {
        while !self.pode_enviar() {
            core::hint::spin_loop();
        }
        unsafe { outb(self.base, byte) };
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // A serial recebe UTF-8 cru: o terminal do outro lado decodifica os acentos.
        for byte in s.bytes() {
            if byte == b'\n' {
                self.escrever_byte(b'\r');
            }
            self.escrever_byte(byte);
        }
        Ok(())
    }
}

pub static SERIAL: Mutex<Serial> = Mutex::new(Serial::new(COM1));

pub fn init() {
    SERIAL.lock().init();
}

// -----------------------------------------------------------------------------
// Recepção
// -----------------------------------------------------------------------------

/// Monta caracteres a partir dos bytes que chegam: o terminal manda UTF-8
/// (um "ç" são 2 bytes) e sequências de escape para teclas como as setas.
struct Decodificador {
    valor: u32,
    faltam: u8,
    escape: u8, // 0: normal, 1: veio ESC, 2: veio ESC [
}

static DECODIFICADOR: Mutex<Decodificador> =
    Mutex::new(Decodificador { valor: 0, faltam: 0, escape: 0 });

impl Decodificador {
    fn receber(&mut self, byte: u8) -> Option<char> {
        match self.escape {
            1 => {
                self.escape = if byte == b'[' { 2 } else { 0 };
                return None;
            }
            2 => {
                // Uma sequência CSI termina num byte entre '@' e '~'. Ignoramos (setas etc.).
                if (0x40..=0x7E).contains(&byte) {
                    self.escape = 0;
                }
                return None;
            }
            _ => {}
        }

        if self.faltam > 0 {
            if byte & 0xC0 == 0x80 {
                self.valor = (self.valor << 6) | (byte & 0x3F) as u32;
                self.faltam -= 1;
                return if self.faltam == 0 { char::from_u32(self.valor) } else { None };
            }
            self.faltam = 0; // sequência UTF-8 quebrada: descarta e segue com este byte
        }

        match byte {
            0x1B => {
                self.escape = 1;
                None
            }
            b'\r' => Some('\n'), // o terminal manda CR no Enter
            0x7F | 0x08 => Some('\u{8}'), // Backspace
            0x00..=0x7F => Some(byte as char),
            0xC0..=0xDF => self.comecar(byte & 0x1F, 1),
            0xE0..=0xEF => self.comecar(byte & 0x0F, 2),
            0xF0..=0xF7 => self.comecar(byte & 0x07, 3),
            _ => None,
        }
    }

    fn comecar(&mut self, bits: u8, faltam: u8) -> Option<char> {
        self.valor = bits as u32;
        self.faltam = faltam;
        None
    }
}

/// Chamada pela IRQ 4. Lê direto da porta, sem o lock de `SERIAL`: quem envia
/// só usa o registrador de transmissão, e este tratador só o de recepção.
pub fn tratar_irq() {
    let mut decodificador = DECODIFICADOR.lock();
    // Bit 0 do Line Status Register: tem byte esperando para ser lido
    while unsafe { inb(COM1 + 5) } & 1 != 0 {
        let byte = unsafe { inb(COM1) };
        if let Some(c) = decodificador.receber(byte) {
            tty::receber(0, c); // a serial é o tty1
        }
    }
}
