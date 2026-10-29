//! Driver de tela em modo texto VGA (80 x 25).
//!
//! Em modo texto, a placa de vídeo lê a tela direto da memória a partir do
//! endereço físico 0xB8000. Cada célula ocupa 2 bytes:
//!
//!   byte 0: o caractere (no conjunto de caracteres "code page 437", não UTF-8!)
//!   byte 1: a cor  -> 4 bits de fundo | 4 bits de frente
//!
//! Escrever na tela é literalmente escrever nesse array.
//!
//! ## Consoles virtuais
//!
//! Há quatro telas (`tty1` a `tty4`, trocadas com Alt+F1..F4). Cada uma
//! guarda o próprio conteúdo na memória; só a ativa é copiada também para a
//! placa de vídeo. Trocar de console é copiar outra tela inteira para lá.
//!
//! Também entendemos um pedaço das sequências de escape ANSI (as mesmas que
//! um terminal Linux entende), para que os programas usem cores e limpem a
//! tela do mesmo jeito na VGA e na serial:
//!
//! ```text
//!   ESC [ 2 J        limpa a tela
//!   ESC [ 0 m        cor padrão
//!   ESC [ 1 m        cores claras
//!   ESC [ 3x m       cor do texto (30 preto ... 37 branco; 90..97 claras)
//! ```

use crate::interrupcoes;
use crate::porta::outb;
use crate::spin::Mutex;
use core::fmt;
use core::ptr::write_volatile;
use core::sync::atomic::{AtomicUsize, Ordering};

const LARGURA: usize = 80;
const ALTURA: usize = 25;
const BUFFER: *mut u16 = 0xB8000 as *mut u16;

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Cor {
    Preto = 0,
    Azul = 1,
    Verde = 2,
    Ciano = 3,
    Vermelho = 4,
    Magenta = 5,
    Marrom = 6,
    CinzaClaro = 7,
    CinzaEscuro = 8,
    AzulClaro = 9,
    VerdeClaro = 10,
    CianoClaro = 11,
    VermelhoClaro = 12,
    Rosa = 13,
    Amarelo = 14,
    Branco = 15,
}

const fn atributo(frente: Cor, fundo: Cor) -> u8 {
    ((fundo as u8) << 4) | (frente as u8)
}

const COR_PADRAO: u8 = atributo(Cor::CinzaClaro, Cor::Preto);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Escape {
    Nenhum,
    Esc,      // veio ESC
    Csi,      // veio ESC [
}

pub const CONSOLES: usize = 4;

/// Qual console está na placa de vídeo agora.
static ATIVA: AtomicUsize = AtomicUsize::new(0);

pub fn ativa() -> usize {
    ATIVA.load(Ordering::Relaxed)
}

pub struct Tela {
    numero: usize,
    celulas: [u16; LARGURA * ALTURA],
    coluna: usize,
    cor: u8,
    escape: Escape,
    parametros: [u16; 4],
    quantos: usize,
}

impl Tela {
    const fn new(numero: usize) -> Self {
        Self {
            numero,
            celulas: [((COR_PADRAO as u16) << 8) | b' ' as u16; LARGURA * ALTURA],
            coluna: 0,
            cor: COR_PADRAO,
            escape: Escape::Nenhum,
            parametros: [0; 4],
            quantos: 0,
        }
    }

    fn visivel(&self) -> bool {
        ativa() == self.numero
    }

    /// Consome um caractere de uma sequência de escape. Devolve `false` se
    /// o caractere não faz parte de nenhuma (e deve ser escrito).
    fn escape(&mut self, c: char) -> bool {
        match self.escape {
            Escape::Nenhum if c == '\x1b' => self.escape = Escape::Esc,
            Escape::Nenhum => return false,
            Escape::Esc if c == '[' => {
                self.escape = Escape::Csi;
                self.parametros = [0; 4];
                self.quantos = 0;
            }
            Escape::Esc => self.escape = Escape::Nenhum,
            Escape::Csi => match c {
                '0'..='9' => {
                    let i = self.quantos.min(3);
                    self.parametros[i] = self.parametros[i].saturating_mul(10) + c as u16 - '0' as u16;
                }
                ';' => self.quantos += 1,
                _ => {
                    self.escape = Escape::Nenhum;
                    let n = (self.quantos + 1).min(4);
                    let parametros = self.parametros;
                    match c {
                        'm' => parametros[..n].iter().for_each(|&p| self.sgr(p)),
                        'J' if parametros[0] == 2 => self.limpar(),
                        _ => {} // movimentos de cursor etc.: ignorados
                    }
                }
            },
        }
        true
    }

    /// "Select Graphic Rendition": muda a cor.
    fn sgr(&mut self, parametro: u16) {
        // ANSI numera preto, vermelho, verde, amarelo, azul, magenta, ciano, branco;
        // a VGA usa outra ordem.
        const ANSI_PARA_VGA: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
        let fundo = self.cor & 0xF0;
        match parametro {
            0 => self.cor = COR_PADRAO,
            1 => self.cor |= 0x08,
            30..=37 => self.cor = fundo | ANSI_PARA_VGA[(parametro - 30) as usize] | (self.cor & 0x08),
            39 => self.cor = fundo | (COR_PADRAO & 0x0F),
            90..=97 => self.cor = fundo | ANSI_PARA_VGA[(parametro - 90) as usize] | 0x08,
            _ => {}
        }
    }

    fn celula(&self, byte: u8) -> u16 {
        ((self.cor as u16) << 8) | byte as u16
    }

    fn por(&mut self, linha: usize, coluna: usize, valor: u16) {
        self.celulas[linha * LARGURA + coluna] = valor;
        if self.visivel() {
            // `write_volatile` impede o compilador de "otimizar fora" a escrita:
            // para ele, ninguém lê essa memória — mas a placa de vídeo lê!
            unsafe { write_volatile(BUFFER.add(linha * LARGURA + coluna), valor) }
        }
    }

    fn ler(&self, linha: usize, coluna: usize) -> u16 {
        self.celulas[linha * LARGURA + coluna]
    }

    /// Copia esta tela inteira para a placa de vídeo (ao trocar de console).
    fn mostrar(&self) {
        for (i, &c) in self.celulas.iter().enumerate() {
            unsafe { write_volatile(BUFFER.add(i), c) };
        }
        self.atualizar_cursor();
    }

    pub fn definir_cor(&mut self, frente: Cor, fundo: Cor) {
        self.cor = atributo(frente, fundo);
    }

    pub fn limpar(&mut self) {
        let vazio = self.celula(b' ');
        for linha in 0..ALTURA {
            for coluna in 0..LARGURA {
                self.por(linha, coluna, vazio);
            }
        }
        self.coluna = 0;
    }

    /// Sempre escrevemos na última linha; ao quebrar a linha, rolamos tudo para cima.
    fn nova_linha(&mut self) {
        for linha in 1..ALTURA {
            for coluna in 0..LARGURA {
                let c = self.ler(linha, coluna);
                self.por(linha - 1, coluna, c);
            }
        }
        let vazio = self.celula(b' ');
        for coluna in 0..LARGURA {
            self.por(ALTURA - 1, coluna, vazio);
        }
        self.coluna = 0;
    }

    pub fn escrever_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.nova_linha(),
            // Backspace só recua o cursor; quem quiser apagar escreve "\x08 \x08".
            0x08 => self.coluna = self.coluna.saturating_sub(1),
            byte => {
                if self.coluna >= LARGURA {
                    self.nova_linha();
                }
                let c = self.celula(byte);
                self.por(ALTURA - 1, self.coluna, c);
                self.coluna += 1;
            }
        }
    }

    /// Move o cursor piscante (desenhado pela própria placa) para onde vai o próximo caractere.
    fn atualizar_cursor(&self) {
        if !self.visivel() {
            return;
        }
        let posicao = ((ALTURA - 1) * LARGURA + self.coluna.min(LARGURA - 1)) as u16;
        unsafe {
            // Registradores do controlador CRT: índice em 0x3D4, valor em 0x3D5.
            outb(0x3D4, 0x0F);
            outb(0x3D5, posicao as u8);
            outb(0x3D4, 0x0E);
            outb(0x3D5, (posicao >> 8) as u8);
        }
    }
}

/// Converte um caractere Unicode para o code page 437 da VGA.
/// O CP437 tem vários acentos (á, é, ó, ç...), mas não tem "ã" nem "õ":
/// esses viram a letra sem acento.
fn para_cp437(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\n' => b'\n',
        '\u{8}' => 0x08,
        'Ç' => 0x80, 'ü' => 0x81, 'é' => 0x82, 'â' => 0x83, 'ä' => 0x84,
        'à' => 0x85, 'ç' => 0x87, 'ê' => 0x88, 'è' => 0x8A, 'É' => 0x90,
        'ô' => 0x93, 'á' => 0xA0, 'í' => 0xA1, 'ó' => 0xA2, 'ú' => 0xA3,
        'ã' | 'Ã' => b'a', 'õ' | 'Õ' => b'o', 'Á' | 'À' | 'Â' => b'A',
        'Í' => b'I', 'Ó' | 'Ô' => b'O', 'Ú' => b'U', 'Ê' => b'E',
        'ñ' => 0xA4, 'Ñ' => 0xA5, 'Ü' => 0x9A, '´' => b'\'', '¨' => b'"',
        '░' => 0xB0, '▒' => 0xB1, '▓' => 0xB2, '│' => 0xB3, '║' => 0xBA,
        '╗' => 0xBB, '╝' => 0xBC, '╚' => 0xC8, '╔' => 0xC9, '═' => 0xCD,
        '█' => 0xDB, '·' => 0xFA, '—' | '–' => b'-', '■' => 0xFE, '→' => 0x1A, '•' => 0x07,
        _ => 0xFE, // caractere desconhecido: um quadradinho
    }
}

impl fmt::Write for Tela {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if !self.escape(c) {
                self.escrever_byte(para_cp437(c));
            }
        }
        self.atualizar_cursor();
        Ok(())
    }
}

pub static TELAS: [Mutex<Tela>; CONSOLES] =
    [Mutex::new(Tela::new(0)), Mutex::new(Tela::new(1)), Mutex::new(Tela::new(2)), Mutex::new(Tela::new(3))];

/// O console das mensagens do kernel (o tty1).
pub static TELA: &Mutex<Tela> = &TELAS[0];

/// Escreve texto num console (visível ou não).
pub fn escrever(console: usize, texto: &str) {
    use core::fmt::Write;
    interrupcoes::sem_interrupcoes(|| {
        let _ = TELAS[console].lock().write_str(texto);
    });
}

/// Troca o console que aparece na tela (Alt+F1..F4).
pub fn ativar(console: usize) {
    if console >= CONSOLES || console == ativa() {
        return;
    }
    interrupcoes::sem_interrupcoes(|| {
        ATIVA.store(console, Ordering::Relaxed);
        TELAS[console].lock().mostrar();
    });
}

/// Limpa a tela e deixa o cursor como uma linha fina (linhas 13 a 15 da célula).
pub fn limpar() {
    interrupcoes::sem_interrupcoes(|| {
        let mut tela = TELA.lock();
        tela.limpar();
        tela.atualizar_cursor();
        unsafe {
            outb(0x3D4, 0x0A); // início do cursor (bit 5 = 0: visível)
            outb(0x3D5, 13);
            outb(0x3D4, 0x0B); // fim do cursor
            outb(0x3D5, 15);
        }
    });
}

pub fn definir_cor(frente: Cor, fundo: Cor) {
    interrupcoes::sem_interrupcoes(|| TELA.lock().definir_cor(frente, fundo));
}
