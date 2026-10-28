//! Modo texto VGA 80x25. A linha 0 é a barra de status; o terminal usa o resto.

use core::fmt;

use crate::cpu::outb;
use crate::paging::PHYS_OFFSET;
use crate::sync::SpinLock;

pub const WIDTH: usize = 80;
pub const HEIGHT: usize = 25;
const TOP: usize = 1;
const BUFFER: *mut u16 = (PHYS_OFFSET + 0xB8000) as *mut u16;

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

impl Color {
    pub const ALL: [Color; 16] = [
        Color::Black,
        Color::Blue,
        Color::Green,
        Color::Cyan,
        Color::Red,
        Color::Magenta,
        Color::Brown,
        Color::LightGray,
        Color::DarkGray,
        Color::LightBlue,
        Color::LightGreen,
        Color::LightCyan,
        Color::LightRed,
        Color::Pink,
        Color::Yellow,
        Color::White,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Color::Black => "preto",
            Color::Blue => "azul",
            Color::Green => "verde",
            Color::Cyan => "ciano",
            Color::Red => "vermelho",
            Color::Magenta => "magenta",
            Color::Brown => "marrom",
            Color::LightGray => "cinza",
            Color::DarkGray => "cinza-escuro",
            Color::LightBlue => "azul-claro",
            Color::LightGreen => "verde-claro",
            Color::LightCyan => "ciano-claro",
            Color::LightRed => "vermelho-claro",
            Color::Pink => "rosa",
            Color::Yellow => "amarelo",
            Color::White => "branco",
        }
    }

    /// Código ANSI equivalente, para espelhar a cor no terminal serial.
    pub fn ansi(self) -> u8 {
        const MAP: [u8; 16] = [30, 34, 32, 36, 31, 35, 33, 37, 90, 94, 92, 96, 91, 95, 93, 97];
        MAP[self as usize]
    }
}

pub const fn attr(fg: Color, bg: Color) -> u8 {
    (bg as u8) << 4 | fg as u8
}

pub const DEFAULT_ATTR: u8 = attr(Color::LightGray, Color::Black);
pub const STATUS_ATTR: u8 = attr(Color::White, Color::Blue);

/// Estado do interpretador de sequências ANSI (ESC [ parâmetros letra).
#[derive(Clone, Copy)]
enum Escape {
    None,
    Started,
    Params { values: [u16; 4], count: usize },
}

pub struct Writer {
    row: usize,
    col: usize,
    attr: u8,
    escape: Escape,
}

pub static WRITER: SpinLock<Writer> =
    SpinLock::new(Writer { row: TOP, col: 0, attr: DEFAULT_ATTR, escape: Escape::None });

pub fn put(row: usize, col: usize, byte: u8, attr: u8) {
    if row < HEIGHT && col < WIDTH {
        unsafe { BUFFER.add(row * WIDTH + col).write_volatile((attr as u16) << 8 | byte as u16) }
    }
}

fn get(row: usize, col: usize) -> u16 {
    unsafe { BUFFER.add(row * WIDTH + col).read_volatile() }
}

/// Escreve um texto na barra de status, a partir da coluna `col`.
pub fn status_text(col: usize, text: &str, attr: u8) {
    for (i, ch) in text.chars().enumerate() {
        put(0, col + i, to_cp437(ch), attr);
    }
}

pub fn init() {
    for col in 0..WIDTH {
        put(0, col, b' ', STATUS_ATTR);
    }
    WRITER.lock().clear();
}

impl Writer {
    pub fn set_attr(&mut self, attr: u8) {
        self.attr = attr;
    }

    pub fn clear(&mut self) {
        for row in TOP..HEIGHT {
            self.clear_row(row);
        }
        self.row = TOP;
        self.col = 0;
        self.update_cursor();
    }

    fn clear_row(&self, row: usize) {
        for col in 0..WIDTH {
            put(row, col, b' ', self.attr);
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < HEIGHT {
            self.row += 1;
        } else {
            for row in TOP + 1..HEIGHT {
                for col in 0..WIDTH {
                    unsafe { BUFFER.add((row - 1) * WIDTH + col).write_volatile(get(row, col)) }
                }
            }
            self.clear_row(HEIGHT - 1);
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => {
                for _ in 0..4 - self.col % 4 {
                    self.write_byte(b' ');
                }
            }
            byte => {
                if self.col >= WIDTH {
                    self.newline();
                }
                put(self.row, self.col, byte, self.attr);
                self.col += 1;
            }
        }
    }

    /// Processa um caractere, interpretando as sequências ANSI mais comuns:
    /// cores (SGR, `ESC[...m`), limpar a tela (`ESC[2J`), cursor ao início
    /// (`ESC[H`) e apagar até o fim da linha (`ESC[K`).
    fn write_char(&mut self, ch: char) {
        match (self.escape, ch) {
            (Escape::None, '\x1b') => self.escape = Escape::Started,
            (Escape::None, ch) => self.write_byte(to_cp437(ch)),
            (Escape::Started, '[') => self.escape = Escape::Params { values: [0; 4], count: 0 },
            (Escape::Started, _) => self.escape = Escape::None,
            (Escape::Params { mut values, count }, '0'..='9') => {
                if count < values.len() {
                    values[count] = values[count].saturating_mul(10) + ch as u16 - '0' as u16;
                }
                self.escape = Escape::Params { values, count };
            }
            (Escape::Params { values, count }, ';') => {
                self.escape = Escape::Params { values, count: (count + 1).min(values.len()) }
            }
            (Escape::Params { values, count }, command) => {
                self.escape = Escape::None;
                self.apply_escape(command, &values[..(count + 1).min(values.len())]);
            }
        }
    }

    fn apply_escape(&mut self, command: char, params: &[u16]) {
        // Ordem ANSI (preto, vermelho, verde, amarelo, azul, magenta, ciano,
        // branco) nos índices de cor da VGA.
        const ANSI_TO_VGA: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
        match command {
            'm' => {
                for &p in params {
                    let p = p as u8;
                    self.attr = match p {
                        0 => DEFAULT_ATTR,
                        1 => self.attr | 0x08,
                        30..=37 => (self.attr & 0xF8) | ANSI_TO_VGA[(p - 30) as usize],
                        90..=97 => (self.attr & 0xF0) | ANSI_TO_VGA[(p - 90) as usize] | 0x08,
                        39 => (self.attr & 0xF0) | (DEFAULT_ATTR & 0x0F),
                        40..=47 => (self.attr & 0x0F) | ANSI_TO_VGA[(p - 40) as usize] << 4,
                        49 => (self.attr & 0x0F) | (DEFAULT_ATTR & 0xF0),
                        _ => self.attr,
                    };
                }
            }
            'J' if params[0] == 2 => self.clear(),
            'H' => {
                self.row = TOP;
                self.col = 0;
            }
            'K' => {
                for col in self.col..WIDTH {
                    put(self.row, col, b' ', self.attr);
                }
            }
            _ => {}
        }
    }

    fn update_cursor(&self) {
        let pos = (self.row * WIDTH + self.col.min(WIDTH - 1)) as u16;
        unsafe {
            outb(0x3D4, 0x0F);
            outb(0x3D5, pos as u8);
            outb(0x3D4, 0x0E);
            outb(0x3D5, (pos >> 8) as u8);
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            self.write_char(ch);
        }
        self.update_cursor();
        Ok(())
    }
}

/// Converte um caractere Unicode para a página de código 437 da VGA.
/// Letras sem equivalente (ã, õ...) perdem o acento.
pub fn to_cp437(ch: char) -> u8 {
    match ch {
        ' '..='~' | '\n' | '\r' | '\t' | '\x08' => ch as u8,
        'Ç' => 0x80,
        'ü' => 0x81,
        'é' => 0x82,
        'â' => 0x83,
        'à' => 0x85,
        'ç' => 0x87,
        'ê' => 0x88,
        'É' => 0x90,
        'ô' => 0x93,
        'á' => 0xA0,
        'í' => 0xA1,
        'ó' => 0xA2,
        'ú' => 0xA3,
        'ã' => b'a',
        'õ' => b'o',
        'Á' | 'Ã' | 'Â' | 'À' => b'A',
        'Ê' => b'E',
        'Í' => b'I',
        'Ó' | 'Õ' | 'Ô' => b'O',
        'Ú' => b'U',
        '░' => 0xB0,
        '▒' => 0xB1,
        '▓' => 0xB2,
        '│' => 0xB3,
        '┐' => 0xBF,
        '└' => 0xC0,
        '─' => 0xC4,
        '┘' => 0xD9,
        '┌' => 0xDA,
        '█' => 0xDB,
        '°' => 0xF8,
        '·' => 0xFA,
        _ => b'?',
    }
}
