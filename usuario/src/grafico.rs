//! Desenho em memória: retângulos, linhas e texto com a fonte 8×16 da VGA.
//!
//! Uma imagem é só um vetor de pixels `0x00RRGGBB`, linha após linha. O
//! servidor de janelas desenha a tela inteira numa imagem dessas e depois a
//! copia para o framebuffer; os programas com janela desenham na imagem que
//! dividem com o servidor (memória compartilhada).

use crate::sys;
use crate::truetype::FonteTT;
use alloc::vec::Vec;

/// Uma imagem para desenhar: `pixels` tem `largura * altura` itens.
pub struct Quadro<'a> {
    pub pixels: &'a mut [u32],
    pub largura: i32,
    pub altura: i32,
}

/// A fonte 8×16 que o kernel leu da VGA (`/dev/fonte`), no code page 437.
pub struct Fonte {
    dados: Vec<u8>,
}

pub const LARGURA_LETRA: i32 = 8;
pub const ALTURA_LETRA: i32 = 16;

impl Fonte {
    pub fn carregar() -> Option<Fonte> {
        let dados = sys::ler_arquivo("/dev/fonte").ok()?;
        (dados.len() == 4096).then_some(Fonte { dados })
    }

    /// As 16 linhas da letra; em cada uma, o bit 7 é o pixel da esquerda.
    pub fn glifo(&self, c: char) -> &[u8] {
        let i = cp437(c) as usize * 16;
        &self.dados[i..i + 16]
    }
}

impl Quadro<'_> {
    pub fn ponto(&mut self, x: i32, y: i32, cor: u32) {
        if x >= 0 && y >= 0 && x < self.largura && y < self.altura {
            self.pixels[(y * self.largura + x) as usize] = cor;
        }
    }

    /// Retângulo cheio (a parte fora da imagem é cortada).
    pub fn retangulo(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: u32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + largura).min(self.largura), (y + altura).min(self.altura));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for linha in y0..y1 {
            let inicio = (linha * self.largura) as usize;
            self.pixels[inicio + x0 as usize..inicio + x1 as usize].fill(cor);
        }
    }

    /// Só a borda de um retângulo (1 pixel).
    pub fn moldura(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: u32) {
        self.retangulo(x, y, largura, 1, cor);
        self.retangulo(x, y + altura - 1, largura, 1, cor);
        self.retangulo(x, y, 1, altura, cor);
        self.retangulo(x + largura - 1, y, 1, altura, cor);
    }

    /// Linha de (x0, y0) a (x1, y1), com `espessura` pixels (algoritmo de Bresenham).
    pub fn linha(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, espessura: i32, cor: u32) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut erro = dx + dy;
        loop {
            self.retangulo(x0 - espessura / 2, y0 - espessura / 2, espessura, espessura, cor);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * erro;
            if e2 >= dy {
                erro += dy;
                x0 += sx;
            }
            if e2 <= dx {
                erro += dx;
                y0 += sy;
            }
        }
    }

    /// Uma letra em (x, y); `fundo` None deixa o fundo como está.
    pub fn letra(&mut self, fonte: &Fonte, x: i32, y: i32, c: char, frente: u32, fundo: Option<u32>) {
        if x + LARGURA_LETRA <= 0 || y + ALTURA_LETRA <= 0 || x >= self.largura || y >= self.altura {
            return;
        }
        for (dy, &bits) in fonte.glifo(c).iter().enumerate() {
            for dx in 0..8 {
                if bits & (0x80 >> dx) != 0 {
                    self.ponto(x + dx, y + dy as i32, frente);
                } else if let Some(f) = fundo {
                    self.ponto(x + dx, y + dy as i32, f);
                }
            }
        }
    }

    pub fn texto(&mut self, fonte: &Fonte, x: i32, y: i32, texto: &str, frente: u32, fundo: Option<u32>) {
        for (i, c) in texto.chars().enumerate() {
            self.letra(fonte, x + i as i32 * LARGURA_LETRA, y, c, frente, fundo);
        }
    }

    /// Mistura `cor` no pixel com a opacidade `alfa` (0 = nada, 255 = cor inteira).
    pub fn misturar(&mut self, x: i32, y: i32, cor: u32, alfa: u8) {
        if x < 0 || y < 0 || x >= self.largura || y >= self.altura || alfa == 0 {
            return;
        }
        let i = (y * self.largura + x) as usize;
        if alfa == 255 {
            self.pixels[i] = cor;
            return;
        }
        let (a, fundo) = (alfa as u32, self.pixels[i]);
        let canal = |d: u32| (((cor >> d) & 0xFF) * a + ((fundo >> d) & 0xFF) * (255 - a)) / 255;
        self.pixels[i] = canal(16) << 16 | canal(8) << 8 | canal(0);
    }

    /// Texto numa fonte TrueType de `tamanho` pixels. `(x, y)` é o canto de
    /// cima à esquerda da linha. Devolve a largura desenhada.
    pub fn texto_tt(&mut self, fonte: &mut FonteTT, x: i32, y: i32, texto: &str, tamanho: u16, cor: u32) -> i32 {
        let (sobe, _) = fonte.metricas(tamanho);
        let base = y + sobe;
        let mut caneta = x * 64; // em 1/64 de pixel: as letras não acumulam arredondamentos
        for c in texto.chars() {
            let g = fonte.glifo(c, tamanho);
            let (gx, gy) = ((caneta + 32) / 64 + g.esquerda, base - g.topo);
            for linha in 0..g.altura {
                for coluna in 0..g.largura {
                    let a = g.alfa[(linha * g.largura + coluna) as usize];
                    self.misturar(gx + coluna, gy + linha, cor, a);
                }
            }
            caneta += g.avanco;
        }
        (caneta + 32) / 64 - x
    }

    /// Copia outra imagem (`largura` × `altura`) para (x, y), cortando o que sobrar.
    pub fn copiar(&mut self, origem: &[u32], largura: i32, altura: i32, x: i32, y: i32) {
        let (x0, x1) = (x.max(0), (x + largura).min(self.largura));
        if x0 >= x1 {
            return;
        }
        for linha in y.max(0)..(y + altura).min(self.altura) {
            let o = ((linha - y) * largura + (x0 - x)) as usize;
            let d = (linha * self.largura + x0) as usize;
            let n = (x1 - x0) as usize;
            self.pixels[d..d + n].copy_from_slice(&origem[o..o + n]);
        }
    }
}

/// O número do caractere na fonte da VGA (a mesma tabela do console do
/// kernel, com as letras acentuadas geradas: veja comum/cp437.rs).
pub fn cp437(c: char) -> u8 {
    crate::cp437::codigo(c)
}
