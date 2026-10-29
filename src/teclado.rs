//! Teclado PS/2, layout ABNT2.
//!
//! A cada tecla apertada ou solta, o controlador 8042 dispara a IRQ 1 e deixa
//! um *scancode* na porta 0x60. A BIOS configura o controlador para traduzir
//! tudo para o "conjunto 1" de scancodes:
//!
//! ```text
//!   apertou: código da tecla (0x01..0x58)       ex.: A = 0x1E
//!   soltou:  código | 0x80                      ex.: A = 0x9E
//!   teclas "novas" (setas, AltGr...): prefixo 0xE0 antes do código
//! ```
//!
//! O scancode diz *qual tecla física* foi apertada, não qual letra. Transformar
//! em letra depende do layout; aqui é o ABNT2, com acentos por teclas mortas
//! (´ ` ~ ^ ¨): a tecla de acento não escreve nada sozinha, ela espera a próxima.

use crate::porta::inb;
use crate::spin::Mutex;
use crate::tty;

const PORTA_DADOS: u16 = 0x60;
const PORTA_STATUS: u16 = 0x64;

struct Estado {
    shift_esquerdo: bool,
    shift_direito: bool,
    ctrl: bool,
    caps_lock: bool,
    estendido: bool,
    acento: Option<char>,
}

// Só o tratador da IRQ 1 mexe nisto, e ele nunca é interrompido: o lock nunca disputa.
static ESTADO: Mutex<Estado> = Mutex::new(Estado {
    shift_esquerdo: false,
    shift_direito: false,
    ctrl: false,
    caps_lock: false,
    estendido: false,
    acento: None,
});

/// Descarta scancodes que tenham ficado no buffer do controlador durante o boot.
pub fn init() {
    unsafe {
        while inb(PORTA_STATUS) & 1 != 0 {
            inb(PORTA_DADOS);
        }
    }
}

pub fn tratar_irq() {
    let scancode = unsafe { inb(PORTA_DADOS) };
    ESTADO.lock().processar(scancode);
}

impl Estado {
    fn processar(&mut self, scancode: u8) {
        if scancode == 0xE0 {
            self.estendido = true;
            return;
        }
        let estendido = core::mem::take(&mut self.estendido);
        let solta = scancode & 0x80 != 0;
        let codigo = scancode & 0x7F;

        if estendido {
            // Do teclado numérico, só o Enter e a barra têm prefixo. Setas,
            // Home, AltGr etc. também chegam aqui, mas ainda não fazem nada.
            match (codigo, solta) {
                (0x1D, _) => self.ctrl = !solta, // Ctrl da direita
                (0x1C, false) => self.digitar('\n'),
                (0x35, false) => self.digitar('/'),
                _ => {}
            }
            return;
        }

        match codigo {
            0x1D => self.ctrl = !solta,
            0x2A => self.shift_esquerdo = !solta,
            0x36 => self.shift_direito = !solta,
            0x3A if !solta => self.caps_lock = !self.caps_lock,
            _ if solta => {}
            _ => {
                if let Some((normal, com_shift)) = mapa_abnt2(codigo) {
                    // Ctrl+letra vira um caractere de controle: Ctrl+C = 3, Ctrl+D = 4...
                    if self.ctrl && normal.is_ascii_lowercase() {
                        tty::receber(((normal as u8) & 0x1F) as char);
                        return;
                    }
                    let shift = self.shift_esquerdo || self.shift_direito;
                    // Caps Lock só vale para letras; Shift inverte o Caps Lock.
                    let maiuscula = if normal.is_alphabetic() { shift != self.caps_lock } else { shift };
                    self.digitar(if maiuscula { com_shift } else { normal });
                }
            }
        }
    }

    fn digitar(&mut self, c: char) {
        if let Some(acento) = self.acento.take() {
            if let Some(composto) = compor(acento, c) {
                tty::receber(composto);
                return;
            }
            // Não combina: sai o acento sozinho. Acento + espaço = só o acento.
            tty::receber(acento);
            if c == ' ' {
                return;
            }
        }
        if matches!(c, '´' | '`' | '~' | '^' | '¨') {
            self.acento = Some(c);
        } else {
            tty::receber(c);
        }
    }
}

/// (sem shift, com shift) de cada tecla, pela posição no teclado ABNT2.
fn mapa_abnt2(codigo: u8) -> Option<(char, char)> {
    const LINHA_Q: &[u8] = b"qwertyuiop";
    const LINHA_A: &[u8] = b"asdfghjkl";
    const LINHA_Z: &[u8] = b"zxcvbnm";
    let letra = |b: u8| (b as char, b.to_ascii_uppercase() as char);

    Some(match codigo {
        0x02 => ('1', '!'),
        0x03 => ('2', '@'),
        0x04 => ('3', '#'),
        0x05 => ('4', '$'),
        0x06 => ('5', '%'),
        0x07 => ('6', '¨'),
        0x08 => ('7', '&'),
        0x09 => ('8', '*'),
        0x0A => ('9', '('),
        0x0B => ('0', ')'),
        0x0C => ('-', '_'),
        0x0D => ('=', '+'),
        0x0E => ('\u{8}', '\u{8}'), // Backspace
        0x0F => ('\t', '\t'),
        0x10..=0x19 => letra(LINHA_Q[(codigo - 0x10) as usize]),
        0x1A => ('´', '`'),
        0x1B => ('[', '{'),
        0x1C => ('\n', '\n'),
        0x1E..=0x26 => letra(LINHA_A[(codigo - 0x1E) as usize]),
        0x27 => ('ç', 'Ç'),
        0x28 => ('~', '^'),
        0x29 => ('\'', '"'),
        0x2B => (']', '}'),
        0x2C..=0x32 => letra(LINHA_Z[(codigo - 0x2C) as usize]),
        0x33 => (',', '<'),
        0x34 => ('.', '>'),
        0x35 => (';', ':'),
        0x37 => ('*', '*'),
        0x39 => (' ', ' '),
        0x4A => ('-', '-'),
        0x4E => ('+', '+'),
        0x56 => ('\\', '|'),
        0x73 => ('/', '?'),
        _ => return None,
    })
}

fn compor(acento: char, letra: char) -> Option<char> {
    Some(match (acento, letra) {
        ('´', 'a') => 'á',
        ('´', 'e') => 'é',
        ('´', 'i') => 'í',
        ('´', 'o') => 'ó',
        ('´', 'u') => 'ú',
        ('´', 'A') => 'Á',
        ('´', 'E') => 'É',
        ('´', 'I') => 'Í',
        ('´', 'O') => 'Ó',
        ('´', 'U') => 'Ú',
        ('`', 'a') => 'à',
        ('`', 'A') => 'À',
        ('~', 'a') => 'ã',
        ('~', 'o') => 'õ',
        ('~', 'n') => 'ñ',
        ('~', 'A') => 'Ã',
        ('~', 'O') => 'Õ',
        ('~', 'N') => 'Ñ',
        ('^', 'a') => 'â',
        ('^', 'e') => 'ê',
        ('^', 'o') => 'ô',
        ('^', 'A') => 'Â',
        ('^', 'E') => 'Ê',
        ('^', 'O') => 'Ô',
        ('¨', 'u') => 'ü',
        ('¨', 'U') => 'Ü',
        _ => return None,
    })
}
