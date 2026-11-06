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
/// Nada é desenhado fora do `recorte` (x, y, largura, altura): é assim que
/// um campo de texto comprido não vaza para fora da sua caixa.
pub struct Quadro<'a> {
    pub pixels: &'a mut [u32],
    pub largura: i32,
    pub altura: i32,
    pub recorte: (i32, i32, i32, i32),
    /// A opacidade de tudo o que passa por `misturar` (255 = normal).
    pub opacidade: u8,
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

impl<'a> Quadro<'a> {
    pub fn novo(pixels: &'a mut [u32], largura: i32, altura: i32) -> Quadro<'a> {
        Quadro { pixels, largura, altura, recorte: (0, 0, largura, altura), opacidade: 255 }
    }

    /// Daqui em diante, só desenha dentro deste retângulo (e dentro da imagem).
    pub fn recortar(&mut self, x: i32, y: i32, largura: i32, altura: i32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + largura).min(self.largura), (y + altura).min(self.altura));
        self.recorte = (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0));
    }

    pub fn sem_recorte(&mut self) {
        self.recorte = (0, 0, self.largura, self.altura);
    }

    fn dentro(&self, x: i32, y: i32) -> bool {
        let (rx, ry, rl, ra) = self.recorte;
        x >= rx && y >= ry && x < rx + rl && y < ry + ra
    }

    pub fn ponto(&mut self, x: i32, y: i32, cor: u32) {
        if self.dentro(x, y) {
            self.pixels[(y * self.largura + x) as usize] = cor;
        }
    }

    /// Retângulo cheio (a parte fora do recorte é cortada).
    pub fn retangulo(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: u32) {
        let (rx, ry, rl, ra) = self.recorte;
        let (x0, y0) = (x.max(rx), y.max(ry));
        let (x1, y1) = ((x + largura).min(rx + rl), (y + altura).min(ry + ra));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for linha in y0..y1 {
            let inicio = (linha * self.largura) as usize;
            self.pixels[inicio + x0 as usize..inicio + x1 as usize].fill(cor);
        }
    }

    /// Retângulo cheio com os cantos arredondados (raio de 3 pixels).
    pub fn retangulo_arredondado(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: u32) {
        // Quantos pixels cada uma das 3 linhas de cima e de baixo perde nos cantos.
        const RECUO: [i32; 3] = [2, 1, 1];
        for (i, &r) in RECUO.iter().enumerate() {
            let i = i as i32;
            self.retangulo(x + r, y + i, largura - 2 * r, 1, cor);
            self.retangulo(x + r, y + altura - 1 - i, largura - 2 * r, 1, cor);
        }
        self.retangulo(x, y + 3, largura, altura - 6, cor);
    }

    /// Círculo cheio de centro (cx, cy).
    pub fn circulo(&mut self, cx: i32, cy: i32, raio: i32, cor: u32) {
        for dy in -raio..=raio {
            // a meia-largura da linha: o maior dx com dx² + dy² ≤ raio²
            let mut dx = 0;
            while (dx + 1) * (dx + 1) + dy * dy <= raio * raio {
                dx += 1;
            }
            self.retangulo(cx - dx, cy + dy, 2 * dx + 1, 1, cor);
        }
    }

    /// Quanto de um pixel fica dentro do círculo de centro (cx, cy) — tudo em
    /// 1/8 de pixel —, testando 16 pontos espalhados nele (0 a 16).
    fn cobertura(px: i32, py: i32, cx8: i32, cy8: i32, raio8: i32) -> i32 {
        let mut dentro = 0;
        for i in 0..4 {
            for j in 0..4 {
                let (dx, dy) = (px * 8 + 2 * i + 1 - cx8, py * 8 + 2 * j + 1 - cy8);
                dentro += (dx * dx + dy * dy <= raio8 * raio8) as i32;
            }
        }
        dentro
    }

    /// Retângulo com cantos redondos de raio qualquer, com as curvas suavizadas.
    pub fn retangulo_redondo(&mut self, x: i32, y: i32, largura: i32, altura: i32, raio: i32, cor: u32) {
        let r = raio.min(largura / 2).min(altura / 2).max(0);
        self.retangulo(x + r, y, largura - 2 * r, altura, cor);
        self.retangulo(x, y + r, r, altura - 2 * r, cor);
        self.retangulo(x + largura - r, y + r, r, altura - 2 * r, cor);
        // Os quatro cantos: cada um, um quarto de círculo.
        let centros = [(x + r, y + r, x, y), (x + largura - r, y + r, x + largura - r, y),
                       (x + r, y + altura - r, x, y + altura - r), (x + largura - r, y + altura - r, x + largura - r, y + altura - r)];
        for (cx, cy, x0, y0) in centros {
            for py in y0..y0 + r {
                for px in x0..x0 + r {
                    let c = Self::cobertura(px, py, cx * 8, cy * 8, r * 8);
                    self.misturar(px, py, cor, (c * 255 / 16) as u8);
                }
            }
        }
    }

    /// Círculo cheio com a borda suavizada; centro e raio em 1/2 pixel
    /// (assim dá para centrar entre dois pixels).
    pub fn circulo_suave(&mut self, cx2: i32, cy2: i32, raio2: i32, cor: u32) {
        self.anel(cx2, cy2, raio2, 0, cor);
    }

    /// Anel (o que fica entre dois círculos), com as bordas suavizadas; em 1/2 pixel.
    pub fn anel(&mut self, cx2: i32, cy2: i32, externo2: i32, interno2: i32, cor: u32) {
        let (cx8, cy8) = (cx2 * 4, cy2 * 4);
        let r = externo2 / 2 + 1;
        for py in cy2 / 2 - r..=cy2 / 2 + r {
            for px in cx2 / 2 - r..=cx2 / 2 + r {
                let fora = Self::cobertura(px, py, cx8, cy8, externo2 * 4);
                let dentro = if interno2 > 0 { Self::cobertura(px, py, cx8, cy8, interno2 * 4) } else { 0 };
                let c = fora - dentro;
                if c > 0 {
                    self.misturar(px, py, cor, (c * 255 / 16) as u8);
                }
            }
        }
    }

    /// Desenha uma imagem (pixels 0xAARRGGBB) esticada ou encolhida para o
    /// retângulo (x, y, largura, altura), respeitando a transparência. Ao
    /// encolher, cada pixel é a média dos pixels da origem que ele cobre (sem
    /// serrilhado); ao esticar, o mais próximo.
    #[allow(clippy::too_many_arguments)]
    pub fn desenhar_imagem(&mut self, origem: &[u32], ow: i32, oh: i32, x: i32, y: i32, largura: i32, altura: i32) {
        if ow <= 0 || oh <= 0 || largura <= 0 || altura <= 0 {
            return;
        }
        let (rx, ry, rl, ra) = self.recorte;
        // Só os pixels de destino que aparecem.
        let (dx0, dx1) = ((rx - x).max(0), (rx + rl - x).min(largura));
        let (dy0, dy1) = ((ry - y).max(0), (ry + ra - y).min(altura));
        for dy in dy0..dy1 {
            let (sy0, sy1) = ((dy as i64 * oh as i64 / altura as i64) as i32, ((dy + 1) as i64 * oh as i64 / altura as i64) as i32);
            let sy1 = sy1.max(sy0 + 1).min(oh);
            for dx in dx0..dx1 {
                let (sx0, sx1) = ((dx as i64 * ow as i64 / largura as i64) as i32, ((dx + 1) as i64 * ow as i64 / largura as i64) as i32);
                let sx1 = sx1.max(sx0 + 1).min(ow);
                let (mut a, mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let p = origem[(sy * ow + sx) as usize];
                        let alfa = p >> 24;
                        a += alfa;
                        // as cores pesam pela opacidade (senão o transparente "suja" a borda)
                        r += (p >> 16 & 0xFF) * alfa;
                        g += (p >> 8 & 0xFF) * alfa;
                        b += (p & 0xFF) * alfa;
                        n += 1;
                    }
                }
                if a == 0 {
                    continue;
                }
                let cor = (r / a) << 16 | (g / a) << 8 | (b / a);
                self.misturar(x + dx, y + dy, cor, (a / n) as u8);
            }
        }
    }

    /// Um retângulo de cor 0xAARRGGBB (com transparência).
    pub fn retangulo_alfa(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: u32) {
        let (rx, ry, rl, ra) = self.recorte;
        let (x0, y0) = (x.max(rx), y.max(ry));
        let (x1, y1) = ((x + largura).min(rx + rl), (y + altura).min(ry + ra));
        let a = (cor >> 24) * self.opacidade as u32 / 255;
        if a == 0 || x1 <= x0 || y1 <= y0 {
            return;
        }
        let rgb = cor & 0xFFFFFF;
        for py in y0..y1 {
            let i = (py * self.largura + x0) as usize;
            let linha = &mut self.pixels[i..i + (x1 - x0) as usize];
            if a == 255 {
                linha.fill(rgb);
            } else {
                for p in linha.iter_mut() {
                    let f = *p;
                    let c = |d: u32| (((rgb >> d) & 0xFF) * a + ((f >> d) & 0xFF) * (255 - a)) / 255;
                    *p = c(16) << 16 | c(8) << 8 | c(0);
                }
            }
        }
    }

    /// Quantos pixels pular em cada lado da linha `dy` de um canto de raio `r`.
    fn recuo_canto(r: i32, dy: i32) -> i32 {
        // dy: a distância (em linhas) até a borda de cima do canto.
        if r <= 0 || dy >= r {
            return 0;
        }
        let d = r - dy;
        let mut x = r;
        while x > 0 && x * x + d * d > r * r {
            x -= 1;
        }
        r - x
    }

    /// Retângulo de cantos redondos (cada canto com seu raio), com transparência.
    pub fn redondo_alfa(&mut self, x: i32, y: i32, l: i32, a: i32, raios: [i32; 4], cor: u32) {
        let [te, td, bd, be] = raios;
        for dy in 0..a {
            let (re, rd) = if dy < a / 2 {
                (Self::recuo_canto(te, dy), Self::recuo_canto(td, dy))
            } else {
                (Self::recuo_canto(be, a - 1 - dy), Self::recuo_canto(bd, a - 1 - dy))
            };
            self.retangulo_alfa(x + re, y + dy, l - re - rd, 1, cor);
        }
    }

    /// As bordas de uma caixa. `estilos`: 1 sólida, 2 tracejada, 3 pontilhada, 4 dupla.
    #[allow(clippy::too_many_arguments)]
    pub fn borda_caixa(&mut self, x: i32, y: i32, l: i32, a: i32, raios: [i32; 4], larguras: [i32; 4], cores: [u32; 4], estilos: [u8; 4]) {
        let [c, d, b, e] = larguras;
        if raios != [0; 4] {
            // Linha a linha: o que está entre o contorno de fora e o de dentro.
            let int = |r: i32, m: i32| (r - m).max(0);
            let [te, td, bd, be] = raios;
            let cor = cores[0];
            for dy in 0..a {
                let (re, rd) = if dy < a / 2 {
                    (Self::recuo_canto(te, dy), Self::recuo_canto(td, dy))
                } else {
                    (Self::recuo_canto(be, a - 1 - dy), Self::recuo_canto(bd, a - 1 - dy))
                };
                let (ini, fim) = (x + re, x + l - rd);
                if dy < c || dy >= a - b {
                    self.retangulo_alfa(ini, y + dy, fim - ini, 1, cor);
                    continue;
                }
                let idy = dy - c;
                let ia = a - c - b;
                let (ie, id) = if idy < ia / 2 {
                    (Self::recuo_canto(int(te, e), idy), Self::recuo_canto(int(td, d), idy))
                } else {
                    (Self::recuo_canto(int(be, e), ia - 1 - idy), Self::recuo_canto(int(bd, d), ia - 1 - idy))
                };
                let (d0, d1) = (x + e + ie, x + l - d - id);
                self.retangulo_alfa(ini, y + dy, d0 - ini, 1, cor);
                self.retangulo_alfa(d1, y + dy, fim - d1, 1, cor);
            }
            return;
        }
        let tracos = |q: &mut Self, x: i32, y: i32, w: i32, h: i32, cor: u32, estilo: u8| {
            let horizontal = w >= h;
            let (passo, cheio) = match estilo {
                2 => (h.min(w).max(1) * 5, h.min(w).max(1) * 3),
                3 => (h.min(w).max(1) * 2, h.min(w).max(1)),
                _ => (0, 0),
            };
            if passo == 0 {
                return q.retangulo_alfa(x, y, w, h, cor);
            }
            let total = if horizontal { w } else { h };
            let mut p = 0;
            while p < total {
                if horizontal {
                    q.retangulo_alfa(x + p, y, cheio.min(total - p), h, cor);
                } else {
                    q.retangulo_alfa(x, y + p, w, cheio.min(total - p), cor);
                }
                p += passo;
            }
        };
        if c > 0 {
            tracos(self, x, y, l, c, cores[0], estilos[0]);
        }
        if b > 0 {
            tracos(self, x, y + a - b, l, b, cores[2], estilos[2]);
        }
        if e > 0 {
            tracos(self, x, y + c, e, a - c - b, cores[3], estilos[3]);
        }
        if d > 0 {
            tracos(self, x + l - d, y + c, d, a - c - b, cores[1], estilos[1]);
        }
    }

    /// Um gradiente linear (`angulo` em graus, como no CSS; paradas de 0 a 1).
    #[allow(clippy::too_many_arguments)]
    pub fn gradiente(&mut self, x: i32, y: i32, l: i32, a: i32, angulo: f32, paradas: &[(f32, u32)]) {
        if l <= 0 || a <= 0 || paradas.is_empty() {
            return;
        }
        let cor_em = |t: f32| -> u32 {
            let t = t.clamp(0.0, 1.0);
            let mut ant = paradas[0];
            if t <= ant.0 {
                return ant.1;
            }
            for p in paradas {
                if t <= p.0 {
                    let k = if p.0 > ant.0 { (t - ant.0) / (p.0 - ant.0) } else { 1.0 };
                    let m = |d: u32| {
                        let (u, v) = (((ant.1 >> d) & 0xFF) as f32, ((p.1 >> d) & 0xFF) as f32);
                        ((u + (v - u) * k) as u32) << d
                    };
                    return m(24) | m(16) | m(8) | m(0);
                }
                ant = *p;
            }
            ant.1
        };
        let graus = crate::mat::fmod(angulo as f64, 360.0);
        let graus = if graus < 0.0 { graus + 360.0 } else { graus };
        if graus == 180.0 || graus == 0.0 {
            for dy in 0..a {
                let t = (dy as f32 + 0.5) / a as f32;
                let t = if graus == 0.0 { 1.0 - t } else { t };
                self.retangulo_alfa(x, y + dy, l, 1, cor_em(t));
            }
            return;
        }
        if graus == 90.0 || graus == 270.0 {
            for dx in 0..l {
                let t = (dx as f32 + 0.5) / l as f32;
                let t = if graus == 270.0 { 1.0 - t } else { t };
                self.retangulo_alfa(x + dx, y, 1, a, cor_em(t));
            }
            return;
        }
        // Outros ângulos: uma tabela de 64 tons (pixel a pixel, em soft-float, é caro).
        let rad = graus * crate::mat::PI / 180.0;
        let (sn, cs) = (crate::mat::sin(rad) as f32, crate::mat::cos(rad) as f32);
        let comp = (l as f32 * sn).abs() + (a as f32 * cs).abs();
        let tons: alloc::vec::Vec<u32> = (0..64).map(|i| cor_em(i as f32 / 63.0)).collect();
        // Fixo em 1/65536: t(dx, dy) = base + dx * px + dy * py.
        let px = (sn / comp * 63.0 * 65536.0) as i32;
        let py = (-cs / comp * 63.0 * 65536.0) as i32;
        let centro = 31.5 * 65536.0;
        let base = (centro - (l as f32 / 2.0) * (px as f32) - (a as f32 / 2.0) * (py as f32)) as i32;
        let (rx, ry, rl, ra) = self.recorte;
        for dy in (ry - y).max(0)..(ry + ra - y).min(a) {
            for dx in (rx - x).max(0)..(rx + rl - x).min(l) {
                let t = (base + dx * px + dy * py) >> 16;
                let c = tons[t.clamp(0, 63) as usize];
                self.misturar(x + dx, y + dy, c & 0xFFFFFF, (c >> 24) as u8);
            }
        }
    }

    /// Uma sombra: camadas cada vez maiores e mais transparentes.
    pub fn sombra(&mut self, x: i32, y: i32, l: i32, a: i32, raios: [i32; 4], borrao: i32, cor: u32) {
        let alfa = cor >> 24;
        if alfa == 0 {
            return;
        }
        let rgb = cor & 0xFFFFFF;
        let passos = (borrao / 2).clamp(0, 8);
        if passos == 0 {
            return self.redondo_alfa(x, y, l, a, raios, cor);
        }
        let cada = (alfa / (passos as u32 + 1)).max(1);
        for i in (0..=passos).rev() {
            let e = i * borrao / (2 * passos) - borrao / 4;
            let r = raios.map(|v| (v + e).max(0));
            self.redondo_alfa(x - e, y - e, l + 2 * e, a + 2 * e, r, cada << 24 | rgb);
        }
    }

    /// Escurece a imagem inteira (o fundo atrás de uma caixa de diálogo).
    pub fn escurecer(&mut self, alfa: u8) {
        for y in 0..self.altura {
            for x in 0..self.largura {
                self.misturar(x, y, 0, alfa);
            }
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
        let (rx, ry, rl, ra) = self.recorte;
        if x + LARGURA_LETRA <= rx || y + ALTURA_LETRA <= ry || x >= rx + rl || y >= ry + ra {
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
        let alfa = if self.opacidade == 255 { alfa } else { (alfa as u32 * self.opacidade as u32 / 255) as u8 };
        if !self.dentro(x, y) || alfa == 0 {
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
        self.texto_tt_estilo(fonte, x, y, texto, tamanho, cor, false)
    }

    /// Como `texto_tt`; com `inclinado`, um itálico "sintético": cada linha
    /// do glifo é empurrada para a direita conforme a altura (1 px a cada 4).
    #[allow(clippy::too_many_arguments)]
    pub fn texto_tt_estilo(&mut self, fonte: &mut FonteTT, x: i32, y: i32, texto: &str, tamanho: u16, cor: u32, inclinado: bool) -> i32 {
        let (sobe, _) = fonte.metricas(tamanho);
        let base = y + sobe;
        let mut caneta = x * 64; // em 1/64 de pixel: as letras não acumulam arredondamentos
        for c in texto.chars() {
            let g = fonte.glifo(c, tamanho);
            let (gx, gy) = ((caneta + 32) / 64 + g.esquerda, base - g.topo);
            for linha in 0..g.altura {
                let empurra = if inclinado { (base - (gy + linha)) / 4 } else { 0 };
                for coluna in 0..g.largura {
                    let a = g.alfa[(linha * g.largura + coluna) as usize];
                    self.misturar(gx + coluna + empurra, gy + linha, cor, a);
                }
            }
            caneta += g.avanco;
        }
        (caneta + 32) / 64 - x
    }

    /// Copia outra imagem (`largura` × `altura`) para (x, y), cortando o que sobrar.
    pub fn copiar(&mut self, origem: &[u32], largura: i32, altura: i32, x: i32, y: i32) {
        let (rx, ry, rl, ra) = self.recorte;
        let (x0, x1) = (x.max(rx), (x + largura).min(rx + rl));
        if x0 >= x1 {
            return;
        }
        for linha in y.max(ry)..(y + altura).min(ry + ra) {
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
