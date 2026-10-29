//! Fontes TrueType: texto de qualquer tamanho, com bordas suaves.
//!
//! Uma fonte TrueType (.ttf) não guarda imagens das letras, e sim o
//! *contorno* de cada uma: retas e curvas (de Bézier quadráticas), numa
//! grade de, por exemplo, 2048 unidades por "em". Para desenhar uma letra em
//! 14 pixels, escalamos o contorno e decidimos, para cada pixel, *quanto* da
//! área dele fica dentro da letra: 100% vira a cor do texto, 30% vira uma
//! mistura de 30% do texto com 70% do fundo. É isso que deixa as bordas
//! suaves (anti-aliasing), em vez de "serrilhadas".
//!
//! O arquivo é uma coleção de tabelas com nomes de 4 letras:
//!
//! ```text
//!   cmap  caractere Unicode → número do glifo
//!   loca  número do glifo → onde está o contorno dele em `glyf`
//!   glyf  os contornos (glifos compostos reusam outros: "ã" = "a" + "~")
//!   hmtx  quanto a caneta anda depois de cada glifo
//!   head, hhea, maxp: unidades por em, altura das letras, quantos glifos...
//! ```
//!
//! O rasterizador é o de "acumulação de área" (o mesmo princípio do font-rs):
//! cada segmento do contorno soma, nos pixels por onde passa, a área
//! (com sinal) que fica à sua direita; uma soma corrida em cada linha dá a
//! cobertura de cada pixel. As letras prontas ficam guardadas por tamanho.

use crate::sys;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

/// Uma letra já rasterizada: a cobertura (0 a 255) de cada pixel.
pub struct Glifo {
    pub largura: i32,
    pub altura: i32,
    /// Onde o canto de cima à esquerda fica em relação à caneta (x) e à linha de base (y, para cima).
    pub esquerda: i32,
    pub topo: i32,
    /// Quanto a caneta anda depois desta letra, em 1/64 de pixel.
    pub avanco: i32,
    pub alfa: Vec<u8>,
}

pub struct FonteTT {
    dados: Vec<u8>,
    unidades: f32,
    loca_longo: bool,
    glifos: u16,
    metricas: u16,
    ascendente: i16,
    descendente: i16,
    espaco_entre_linhas: i16,
    cmap: (usize, u16), // (onde, formato)
    loca: usize,
    glyf: usize,
    hmtx: usize,
    prontos: BTreeMap<(u16, u16), Glifo>,
}

fn piso(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i > x { i - 1.0 } else { i }
}

fn teto(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i < x { i + 1.0 } else { i }
}

#[derive(Clone, Copy)]
struct Ponto {
    x: f32,
    y: f32,
}

impl FonteTT {
    fn u16_em(&self, o: usize) -> u16 {
        self.dados.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16_em(&self, o: usize) -> i16 {
        self.u16_em(o) as i16
    }
    fn u32_em(&self, o: usize) -> u32 {
        self.dados.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn carregar(caminho: &str) -> Option<FonteTT> {
        Self::de_bytes(sys::ler_arquivo(caminho).ok()?)
    }

    pub fn de_bytes(dados: Vec<u8>) -> Option<FonteTT> {
        let mut f = FonteTT {
            dados,
            unidades: 0.0,
            loca_longo: false,
            glifos: 0,
            metricas: 0,
            ascendente: 0,
            descendente: 0,
            espaco_entre_linhas: 0,
            cmap: (0, 0),
            loca: 0,
            glyf: 0,
            hmtx: 0,
            prontos: BTreeMap::new(),
        };
        // 0x00010000 = TrueType com contornos `glyf` (as de CFF, "OTTO", não servem aqui).
        if f.u32_em(0) != 0x0001_0000 {
            return None;
        }
        let tabela = |nome: &[u8; 4]| -> Option<usize> {
            (0..f.u16_em(4) as usize)
                .map(|i| 12 + i * 16)
                .find(|&o| f.dados.get(o..o + 4) == Some(nome))
                .map(|o| f.u32_em(o + 8) as usize)
        };
        let (head, hhea, maxp, cmap) = (tabela(b"head")?, tabela(b"hhea")?, tabela(b"maxp")?, tabela(b"cmap")?);
        let (loca, glyf, hmtx) = (tabela(b"loca")?, tabela(b"glyf")?, tabela(b"hmtx")?);
        f.unidades = f.u16_em(head + 18) as f32;
        f.loca_longo = f.i16_em(head + 50) != 0;
        f.glifos = f.u16_em(maxp + 4);
        f.ascendente = f.i16_em(hhea + 4);
        f.descendente = f.i16_em(hhea + 6);
        f.espaco_entre_linhas = f.i16_em(hhea + 8);
        f.metricas = f.u16_em(hhea + 34);
        (f.loca, f.glyf, f.hmtx) = (loca, glyf, hmtx);
        // A subtabela do cmap: Unicode completo (formato 12) ou só o básico (formato 4).
        let mut escolhida = None;
        for i in 0..f.u16_em(cmap + 2) as usize {
            let r = cmap + 4 + i * 8;
            let (plataforma, codificacao) = (f.u16_em(r), f.u16_em(r + 2));
            let onde = cmap + f.u32_em(r + 4) as usize;
            let formato = f.u16_em(onde);
            let unicode = plataforma == 0 || plataforma == 3 && (codificacao == 1 || codificacao == 10);
            if unicode && (formato == 12 || formato == 4 && escolhida.is_none()) {
                escolhida = Some((onde, formato));
            }
        }
        f.cmap = escolhida?;
        (f.unidades > 0.0).then_some(f)
    }

    /// O número do glifo de um caractere (0 = o glifo de "não existe").
    pub fn indice(&self, c: char) -> u16 {
        let c = c as u32;
        let (o, formato) = self.cmap;
        if formato == 12 {
            for g in 0..self.u32_em(o + 12) as usize {
                let r = o + 16 + g * 12;
                let (inicio, fim) = (self.u32_em(r), self.u32_em(r + 4));
                if (inicio..=fim).contains(&c) {
                    return (self.u32_em(r + 8) + c - inicio) as u16;
                }
            }
            return 0;
        }
        if c > 0xFFFF {
            return 0;
        }
        let segmentos = self.u16_em(o + 6) as usize / 2;
        let fins = o + 14;
        let inicios = fins + segmentos * 2 + 2;
        let deltas = inicios + segmentos * 2;
        let saltos = deltas + segmentos * 2;
        for s in 0..segmentos {
            if c as u16 > self.u16_em(fins + s * 2) {
                continue;
            }
            let inicio = self.u16_em(inicios + s * 2);
            if (c as u16) < inicio {
                return 0;
            }
            let delta = self.u16_em(deltas + s * 2);
            let salto = self.u16_em(saltos + s * 2);
            if salto == 0 {
                return (c as u16).wrapping_add(delta);
            }
            let g = self.u16_em(saltos + s * 2 + salto as usize + (c as usize - inicio as usize) * 2);
            return if g == 0 { 0 } else { g.wrapping_add(delta) };
        }
        0
    }

    fn onde_esta(&self, glifo: u16) -> Option<(usize, usize)> {
        if glifo >= self.glifos {
            return None;
        }
        let (a, b) = if self.loca_longo {
            (self.u32_em(self.loca + glifo as usize * 4) as usize, self.u32_em(self.loca + glifo as usize * 4 + 4) as usize)
        } else {
            (self.u16_em(self.loca + glifo as usize * 2) as usize * 2, self.u16_em(self.loca + glifo as usize * 2 + 2) as usize * 2)
        };
        (b > a).then_some((self.glyf + a, b - a))
    }

    fn avanco_em_unidades(&self, glifo: u16) -> u16 {
        let i = (glifo.min(self.metricas.saturating_sub(1))) as usize;
        self.u16_em(self.hmtx + i * 4)
    }

    /// Os contornos de um glifo, em unidades da fonte: listas de (ponto, está na curva?).
    fn contornos(&self, glifo: u16, profundidade: u32, saida: &mut Vec<Vec<(Ponto, bool)>>) {
        let Some((o, _)) = self.onde_esta(glifo) else { return };
        let n = self.i16_em(o);
        if n >= 0 {
            self.simples(o, n as usize, saida);
        } else if profundidade < 8 {
            self.composto(o, profundidade, saida);
        }
    }

    fn simples(&self, o: usize, n: usize, saida: &mut Vec<Vec<(Ponto, bool)>>) {
        let fins: Vec<usize> = (0..n).map(|i| self.u16_em(o + 10 + i * 2) as usize).collect();
        let total = fins.last().map_or(0, |&f| f + 1);
        let instrucoes = self.u16_em(o + 10 + n * 2) as usize;
        let mut p = o + 12 + n * 2 + instrucoes;
        // As flags (com repetição), depois todos os x, depois todos os y.
        let mut flags = Vec::with_capacity(total);
        while flags.len() < total {
            let f = *self.dados.get(p).unwrap_or(&0);
            p += 1;
            flags.push(f);
            if f & 8 != 0 {
                let vezes = *self.dados.get(p).unwrap_or(&0);
                p += 1;
                for _ in 0..vezes {
                    flags.push(f);
                }
            }
        }
        flags.truncate(total);
        let mut ler_eixo = |curto: u8, igual: u8| -> Vec<i32> {
            let mut v = Vec::with_capacity(total);
            let mut atual = 0i32;
            for &f in &flags {
                if f & curto != 0 {
                    let d = *self.dados.get(p).unwrap_or(&0) as i32;
                    p += 1;
                    atual += if f & igual != 0 { d } else { -d };
                } else if f & igual == 0 {
                    atual += self.i16_em(p) as i32;
                    p += 2;
                }
                v.push(atual);
            }
            v
        };
        let xs = ler_eixo(2, 16);
        let ys = ler_eixo(4, 32);
        let mut inicio = 0;
        for &fim in &fins {
            if fim >= total || fim < inicio {
                break;
            }
            saida.push(
                (inicio..=fim).map(|i| (Ponto { x: xs[i] as f32, y: ys[i] as f32 }, flags[i] & 1 != 0)).collect(),
            );
            inicio = fim + 1;
        }
    }

    /// Um glifo feito de outros (com deslocamento e, às vezes, escala).
    fn composto(&self, mut o: usize, profundidade: u32, saida: &mut Vec<Vec<(Ponto, bool)>>) {
        o += 10;
        loop {
            let flags = self.u16_em(o);
            let componente = self.u16_em(o + 2);
            o += 4;
            let (dx, dy) = if flags & 1 != 0 {
                let r = (self.i16_em(o) as f32, self.i16_em(o + 2) as f32);
                o += 4;
                r
            } else {
                let r = (self.dados[o] as i8 as f32, self.dados[o + 1] as i8 as f32);
                o += 2;
                r
            };
            let f2dot14 = |v: i16| v as f32 / 16384.0;
            let (mut a, mut b, mut c, mut d) = (1.0, 0.0, 0.0, 1.0);
            if flags & 8 != 0 {
                a = f2dot14(self.i16_em(o));
                d = a;
                o += 2;
            } else if flags & 0x40 != 0 {
                (a, d) = (f2dot14(self.i16_em(o)), f2dot14(self.i16_em(o + 2)));
                o += 4;
            } else if flags & 0x80 != 0 {
                (a, b, c, d) = (
                    f2dot14(self.i16_em(o)),
                    f2dot14(self.i16_em(o + 2)),
                    f2dot14(self.i16_em(o + 4)),
                    f2dot14(self.i16_em(o + 6)),
                );
                o += 8;
            }
            let mut partes = Vec::new();
            self.contornos(componente, profundidade + 1, &mut partes);
            // Só deslocamentos em x/y (ARGS_ARE_XY_VALUES); o outro modo, por pontos, é raro.
            let (dx, dy) = if flags & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
            for contorno in partes {
                saida.push(
                    contorno
                        .into_iter()
                        .map(|(p, na_curva)| (Ponto { x: a * p.x + c * p.y + dx, y: b * p.x + d * p.y + dy }, na_curva))
                        .collect(),
                );
            }
            if flags & 0x20 == 0 {
                break; // não há mais componentes
            }
        }
    }

    /// A letra `c` em `tamanho` pixels (altura do "em"), rasterizada (e guardada).
    pub fn glifo(&mut self, c: char, tamanho: u16) -> &Glifo {
        let indice = self.indice(c);
        if !self.prontos.contains_key(&(indice, tamanho)) {
            let g = self.rasterizar(indice, tamanho);
            self.prontos.insert((indice, tamanho), g);
        }
        &self.prontos[&(indice, tamanho)]
    }

    fn rasterizar(&self, indice: u16, tamanho: u16) -> Glifo {
        let escala = tamanho as f32 / self.unidades;
        let avanco = (self.avanco_em_unidades(indice) as f32 * escala * 64.0) as i32;
        let mut contornos = Vec::new();
        self.contornos(indice, 0, &mut contornos);
        let pontos = contornos.iter().flatten();
        let (mut xmin, mut ymin, mut xmax, mut ymax) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (p, _) in pontos {
            (xmin, xmax) = (xmin.min(p.x), xmax.max(p.x));
            (ymin, ymax) = (ymin.min(p.y), ymax.max(p.y));
        }
        if contornos.is_empty() || xmin > xmax {
            return Glifo { largura: 0, altura: 0, esquerda: 0, topo: 0, avanco, alfa: Vec::new() };
        }
        // A grade de pixels: um de folga em volta.
        let esquerda = piso(xmin * escala) - 1.0;
        let topo = teto(ymax * escala) + 1.0;
        let largura = (teto(xmax * escala) - esquerda + 1.0) as usize;
        let altura = (topo - piso(ymin * escala) + 1.0) as usize;
        let mut r = Rasterizador::novo(largura, altura);
        let transformar = |p: Ponto| Ponto { x: p.x * escala - esquerda, y: topo - p.y * escala };
        for contorno in &contornos {
            r.contorno(contorno, &transformar);
        }
        Glifo {
            largura: largura as i32,
            altura: altura as i32,
            esquerda: esquerda as i32,
            topo: topo as i32,
            avanco,
            alfa: r.cobertura(),
        }
    }

    /// Quanto o texto ocupa na horizontal, em pixels.
    pub fn largura(&mut self, texto: &str, tamanho: u16) -> i32 {
        let mut total = 0;
        for c in texto.chars() {
            total += self.glifo(c, tamanho).avanco;
        }
        (total + 32) / 64
    }

    /// (quanto sobe acima da linha de base, altura de uma linha) em pixels.
    pub fn metricas(&self, tamanho: u16) -> (i32, i32) {
        let escala = tamanho as f32 / self.unidades;
        let sobe = teto(self.ascendente as f32 * escala) as i32;
        let linha = teto((self.ascendente - self.descendente + self.espaco_entre_linhas) as f32 * escala) as i32;
        (sobe, linha)
    }
}

/// O acumulador de área: para cada pixel, a área assinada que os segmentos
/// deixam à sua direita; a soma corrida de cada linha dá a cobertura.
struct Rasterizador {
    largura: usize,
    altura: usize,
    acumulado: Vec<f32>,
}

impl Rasterizador {
    fn novo(largura: usize, altura: usize) -> Self {
        Rasterizador { largura, altura, acumulado: vec![0.0; largura * altura + 4] }
    }

    fn somar(&mut self, i: isize, v: f32) {
        if i >= 0 && (i as usize) < self.acumulado.len() {
            self.acumulado[i as usize] += v;
        }
    }

    fn reta(&mut self, a: Ponto, b: Ponto) {
        if a.y == b.y {
            return;
        }
        let (direcao, a, b) = if a.y < b.y { (1.0, a, b) } else { (-1.0, b, a) };
        let dxdy = (b.x - a.x) / (b.y - a.y);
        let mut x = a.x;
        let y0 = if a.y < 0.0 {
            x -= a.y * dxdy;
            0
        } else {
            a.y as usize
        };
        let y1 = (teto(b.y) as usize).min(self.altura);
        for y in y0..y1 {
            let linha = (y * self.largura) as isize;
            let dy = ((y + 1) as f32).min(b.y) - (y as f32).max(a.y);
            let proximo = x + dxdy * dy;
            let d = dy * direcao;
            let (x0, x1) = if x < proximo { (x, proximo) } else { (proximo, x) };
            let x0_piso = piso(x0);
            let x0i = x0_piso as isize;
            let x1_teto = teto(x1);
            let x1i = x1_teto as isize;
            if x1i <= x0i + 1 {
                // A reta cruza um pixel só nesta linha.
                let meio = 0.5 * (x + proximo) - x0_piso;
                self.somar(linha + x0i, d - d * meio);
                self.somar(linha + x0i + 1, d * meio);
            } else {
                let s = 1.0 / (x1 - x0);
                let x0f = x0 - x0_piso;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1_teto + 1.0;
                let am = 0.5 * s * x1f * x1f;
                self.somar(linha + x0i, d * a0);
                if x1i == x0i + 2 {
                    self.somar(linha + x0i + 1, d * (1.0 - a0 - am));
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.somar(linha + x0i + 1, d * (a1 - a0));
                    for xi in x0i + 2..x1i - 1 {
                        self.somar(linha + xi, d * s);
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.somar(linha + x1i - 1, d * (1.0 - a2 - am));
                }
                self.somar(linha + x1i, d * am);
            }
            x = proximo;
        }
    }

    /// Curva quadrática de Bézier, quebrada em retas (mais pedaços se for mais curva).
    fn curva(&mut self, a: Ponto, controle: Ponto, b: Ponto) {
        let desvio = (a.x - 2.0 * controle.x + b.x).abs() + (a.y - 2.0 * controle.y + b.y).abs();
        let pedacos = (desvio * 0.6) as usize + 1;
        let pedacos = pedacos.min(16);
        let mut anterior = a;
        for i in 1..=pedacos {
            let t = i as f32 / pedacos as f32;
            let u = 1.0 - t;
            let p = Ponto {
                x: u * u * a.x + 2.0 * u * t * controle.x + t * t * b.x,
                y: u * u * a.y + 2.0 * u * t * controle.y + t * t * b.y,
            };
            self.reta(anterior, p);
            anterior = p;
        }
    }

    /// Um contorno do TrueType: dois pontos seguidos fora da curva têm um
    /// ponto implícito na curva no meio deles.
    fn contorno(&mut self, pontos: &[(Ponto, bool)], transformar: &impl Fn(Ponto) -> Ponto) {
        let n = pontos.len();
        if n < 2 {
            return;
        }
        let meio = |a: Ponto, b: Ponto| Ponto { x: (a.x + b.x) / 2.0, y: (a.y + b.y) / 2.0 };
        let p = |i: usize| (transformar(pontos[i % n].0), pontos[i % n].1);
        // Começa num ponto da curva (ou no meio dos dois primeiros, se não houver).
        let primeiro = (0..n).find(|&i| pontos[i].1);
        let (inicio, mut atual, deslocamento) = match primeiro {
            Some(i) => (p(i).0, p(i).0, i),
            None => {
                let m = meio(p(0).0, p(1).0);
                (m, m, 1)
            }
        };
        let mut controle: Option<Ponto> = None;
        for k in 1..=n {
            let (q, na_curva) = p(deslocamento + k);
            match (na_curva, controle) {
                (true, None) => {
                    self.reta(atual, q);
                    atual = q;
                }
                (true, Some(c)) => {
                    self.curva(atual, c, q);
                    atual = q;
                    controle = None;
                }
                (false, None) => controle = Some(q),
                (false, Some(c)) => {
                    let m = meio(c, q);
                    self.curva(atual, c, m);
                    atual = m;
                    controle = Some(q);
                }
            }
        }
        match controle {
            Some(c) => self.curva(atual, c, inicio),
            None => self.reta(atual, inicio),
        }
    }

    fn cobertura(&self) -> Vec<u8> {
        let mut soma = 0.0f32;
        let mut alfa = Vec::with_capacity(self.largura * self.altura);
        for i in 0..self.largura * self.altura {
            soma += self.acumulado[i];
            let a = soma.abs().min(1.0);
            alfa.push((a * 255.0 + 0.5) as u8);
        }
        alfa
    }
}
