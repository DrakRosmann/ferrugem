//! Decodificador de JPEG (ITU T.81): o "baseline" e o progressivo, que são
//! quase todas as fotos da web.
//!
//! ```text
//!   bytes ─▶ Huffman ─▶ coeficientes de cada bloco 8×8 (por componente)
//!         ─▶ × tabela de quantização ─▶ IDCT (frequências → pixels)
//!         ─▶ Y, Cb, Cr (a cor em resolução menor) ─▶ RGB
//! ```
//!
//! O baseline manda cada bloco inteiro de uma vez. O progressivo manda a
//! imagem várias vezes ("scans"): primeiro só a média de cada bloco, depois
//! as frequências baixas, as altas, e os bits menos significativos por
//! último; por isso guardamos os coeficientes de todos os blocos e só no
//! fim viramos pixels.
//!
//! Não lemos: JPEG aritmético, sem perdas, 12 bits e CMYK (raros na web).

use crate::png::Imagem;
use alloc::vec;
use alloc::vec::Vec;

/// A ordem em zigue-zague: a posição k do arquivo é o coeficiente ZIGUE[k] do bloco.
const ZIGUE: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55,
    62, 63,
];

/// C(u)/2 · cos((2x+1)uπ/16), × 4096: a IDCT de uma dimensão (C(0) = 1/√2).
const COSSENOS: [[i64; 8]; 8] = [
    [1448, 2009, 1892, 1703, 1448, 1138, 784, 400],
    [1448, 1703, 784, -400, -1448, -2009, -1892, -1138],
    [1448, 1138, -784, -2009, -1448, 400, 1892, 1703],
    [1448, 400, -1892, -1138, 1448, 1703, -784, -2009],
    [1448, -400, -1892, 1138, 1448, -1703, -784, 2009],
    [1448, -1138, -784, 2009, -1448, -400, 1892, -1703],
    [1448, -1703, 784, 400, -1448, 2009, -1892, 1138],
    [1448, -2009, 1892, -1703, 1448, -1138, 784, -400],
];

/// Uma tabela de Huffman (códigos canônicos: só os tamanhos e os símbolos).
#[derive(Clone, Default)]
struct Huffman {
    /// Os primeiros 9 bits → (tamanho do código, símbolo); tamanho 0 = código mais longo.
    rapida: Vec<(u8, u8)>,
    /// Para os códigos longos: por tamanho, o maior código e onde começam os símbolos.
    maior: [i32; 17],
    inicio: [i32; 17],
    simbolos: Vec<u8>,
}

impl Huffman {
    fn nova(contagens: &[u8], simbolos: &[u8]) -> Huffman {
        let mut h = Huffman { rapida: vec![(0, 0); 512], maior: [-1; 17], inicio: [0; 17], simbolos: simbolos.to_vec() };
        let (mut codigo, mut k) = (0i32, 0usize);
        for tamanho in 1..=16 {
            let n = contagens[tamanho - 1] as usize;
            h.inicio[tamanho] = k as i32 - codigo;
            for _ in 0..n {
                if tamanho <= 9 && k < simbolos.len() {
                    // todas as continuações de 9 bits deste código
                    let base = (codigo << (9 - tamanho)) as usize;
                    for c in base..base + (1 << (9 - tamanho)) {
                        if c < 512 {
                            h.rapida[c] = (tamanho as u8, simbolos[k]);
                        }
                    }
                }
                codigo += 1;
                k += 1;
            }
            if n > 0 {
                h.maior[tamanho] = codigo - 1;
            }
            codigo <<= 1;
        }
        h
    }
}

/// Os bits dos dados entrópicos: com o "0xFF 0x00" (um 0xFF de verdade)
/// desfeito, e parando nos marcadores.
struct Bits<'a> {
    dados: &'a [u8],
    pos: usize,
    acumulado: u32,
    quantos: u32,
    /// Chegou num marcador (o fim do scan, ou um RST): daqui só zeros.
    marcador: bool,
}

impl Bits<'_> {
    fn encher(&mut self) {
        while self.quantos <= 24 {
            let mut b = 0;
            if !self.marcador && self.pos < self.dados.len() {
                b = self.dados[self.pos];
                if b == 0xFF {
                    match self.dados.get(self.pos + 1) {
                        Some(0) => self.pos += 2,
                        _ => {
                            self.marcador = true;
                            b = 0;
                        }
                    }
                } else {
                    self.pos += 1;
                }
            }
            self.acumulado |= (b as u32) << (24 - self.quantos);
            self.quantos += 8;
        }
    }

    fn ler(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.encher();
        let v = self.acumulado >> (32 - n);
        self.acumulado <<= n;
        self.quantos -= n;
        v
    }

    fn bit(&mut self) -> bool {
        self.ler(1) == 1
    }

    /// Um valor de `s` bits com sinal (a "extensão" do JPEG: 0xxx é negativo).
    fn valor(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.ler(s) as i32;
        if v < 1 << (s - 1) { v - (1 << s) + 1 } else { v }
    }

    fn simbolo(&mut self, h: &Huffman) -> Option<u8> {
        if h.rapida.is_empty() {
            return None; // uma tabela que o arquivo não definiu
        }
        self.encher();
        let (tamanho, s) = h.rapida[(self.acumulado >> 23) as usize];
        if tamanho > 0 {
            self.acumulado <<= tamanho;
            self.quantos -= tamanho as u32;
            return Some(s);
        }
        let mut codigo = 0i32;
        for tamanho in 1..=16 {
            codigo = codigo << 1 | self.ler(1) as i32;
            if codigo <= h.maior[tamanho] {
                return h.simbolos.get((h.inicio[tamanho] + codigo) as usize).copied();
            }
        }
        None
    }

    /// Depois de um intervalo de reinício: pula o marcador RSTn.
    fn reiniciar(&mut self) {
        self.acumulado = 0;
        self.quantos = 0;
        while self.pos + 1 < self.dados.len() && !(self.dados[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.dados[self.pos + 1])) {
            self.pos += 1;
        }
        self.pos = (self.pos + 2).min(self.dados.len());
        self.marcador = false;
    }
}

struct Componente {
    id: u8,
    h: usize,
    v: usize,
    quantizacao: usize,
    /// Blocos por linha e por coluna (arredondados para MCUs inteiras).
    blocos_l: usize,
    blocos_a: usize,
    /// Os blocos que de fato cobrem a imagem (num scan só deste componente).
    uteis_l: usize,
    uteis_a: usize,
    coeficientes: Vec<i16>,
    dc: usize,
    ac: usize,
    previsto: i32,
}

struct Jpeg {
    largura: usize,
    altura: usize,
    progressivo: bool,
    componentes: Vec<Componente>,
    quantizacao: [[u16; 64]; 4],
    dc: [Huffman; 4],
    ac: [Huffman; 4],
    reinicio: usize,
    hmax: usize,
    vmax: usize,
    /// Quantos blocos seguidos ainda são só zeros (o "EOBRUN" do progressivo).
    fim_de_blocos: u32,
    adobe_rgb: bool,
}

fn u16_em(d: &[u8], i: usize) -> Option<usize> {
    Some((*d.get(i)? as usize) << 8 | *d.get(i + 1)? as usize)
}

pub fn decodificar(dados: &[u8]) -> Option<Imagem> {
    if dados.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut j = Jpeg {
        largura: 0,
        altura: 0,
        progressivo: false,
        componentes: Vec::new(),
        quantizacao: [[1; 64]; 4],
        dc: Default::default(),
        ac: Default::default(),
        reinicio: 0,
        hmax: 1,
        vmax: 1,
        fim_de_blocos: 0,
        adobe_rgb: false,
    };
    let mut i = 2;
    loop {
        // Os marcadores: 0xFF (talvez repetido) + o tipo.
        while *dados.get(i)? != 0xFF {
            i += 1;
        }
        while *dados.get(i)? == 0xFF {
            i += 1;
        }
        let tipo = *dados.get(i)?;
        i += 1;
        if tipo == 0xD9 {
            break; // EOI
        }
        if (0xD0..=0xD7).contains(&tipo) || tipo == 0x01 {
            continue;
        }
        let tamanho = u16_em(dados, i)?;
        let seg = dados.get(i + 2..i + tamanho)?;
        match tipo {
            0xC0 | 0xC1 | 0xC2 => {
                j.progressivo = tipo == 0xC2;
                if *seg.first()? != 8 {
                    return None; // 12 bits
                }
                j.altura = u16_em(seg, 1)?;
                j.largura = u16_em(seg, 3)?;
                let n = *seg.get(5)? as usize;
                if j.largura == 0 || j.altura == 0 || j.largura * j.altura > 16 << 20 || !(n == 1 || n == 3) {
                    return None;
                }
                for c in 0..n {
                    let d = seg.get(6 + c * 3..9 + c * 3)?;
                    let (h, v) = ((d[1] >> 4) as usize, (d[1] & 15) as usize);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) || d[2] > 3 {
                        return None;
                    }
                    j.componentes.push(Componente {
                        id: d[0],
                        h,
                        v,
                        quantizacao: d[2] as usize,
                        blocos_l: 0,
                        blocos_a: 0,
                        uteis_l: 0,
                        uteis_a: 0,
                        coeficientes: Vec::new(),
                        dc: 0,
                        ac: 0,
                        previsto: 0,
                    });
                }
                j.hmax = j.componentes.iter().map(|c| c.h).max()?;
                j.vmax = j.componentes.iter().map(|c| c.v).max()?;
                let mcus_l = j.largura.div_ceil(8 * j.hmax);
                let mcus_a = j.altura.div_ceil(8 * j.vmax);
                for c in j.componentes.iter_mut() {
                    c.blocos_l = mcus_l * c.h;
                    c.blocos_a = mcus_a * c.v;
                    c.uteis_l = (j.largura * c.h).div_ceil(j.hmax).div_ceil(8);
                    c.uteis_a = (j.altura * c.v).div_ceil(j.vmax).div_ceil(8);
                    c.coeficientes = vec![0; c.blocos_l * c.blocos_a * 64];
                }
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None, // sem perdas, aritmético
            0xC4 => {
                let mut k = 0;
                while k + 17 <= seg.len() {
                    let (classe, id) = (seg[k] >> 4, (seg[k] & 15) as usize);
                    let contagens = &seg[k + 1..k + 17];
                    let total: usize = contagens.iter().map(|&c| c as usize).sum();
                    let simbolos = seg.get(k + 17..k + 17 + total)?;
                    if id > 3 {
                        return None;
                    }
                    let h = Huffman::nova(contagens, simbolos);
                    if classe == 0 { j.dc[id] = h } else { j.ac[id] = h }
                    k += 17 + total;
                }
            }
            0xDB => {
                let mut k = 0;
                while k < seg.len() {
                    let (precisao, id) = (seg[k] >> 4, (seg[k] & 15) as usize);
                    if id > 3 {
                        return None;
                    }
                    for n in 0..64 {
                        j.quantizacao[id][n] = if precisao == 0 { *seg.get(k + 1 + n)? as u16 } else { u16_em(seg, k + 1 + 2 * n)? as u16 };
                    }
                    k += 1 + if precisao == 0 { 64 } else { 128 };
                }
            }
            0xDD => j.reinicio = u16_em(seg, 0)?,
            0xEE if seg.starts_with(b"Adobe") => j.adobe_rgb = seg.get(11) == Some(&0),
            0xDA => {
                if j.componentes.is_empty() {
                    return None;
                }
                let fim = j.scan(seg, &dados[i + tamanho..])?;
                i += tamanho + fim;
                continue;
            }
            _ => {} // APPn, comentários...
        }
        i += tamanho;
    }
    j.pixels()
}

impl Jpeg {
    /// Um scan: `seg` é o cabeçalho (SOS), `dados` o que vem depois. Devolve
    /// quantos bytes de dados foram usados.
    fn scan(&mut self, seg: &[u8], dados: &[u8]) -> Option<usize> {
        let n = *seg.first()? as usize;
        let mut nos = Vec::new();
        for c in 0..n {
            let (id, tabelas) = (*seg.get(1 + c * 2)?, *seg.get(2 + c * 2)?);
            let i = self.componentes.iter().position(|c| c.id == id)?;
            self.componentes[i].dc = (tabelas >> 4) as usize & 3;
            self.componentes[i].ac = (tabelas & 15) as usize & 3;
            nos.push(i);
        }
        let (inicio, fim) = (*seg.get(1 + n * 2)? as usize, *seg.get(2 + n * 2)? as usize);
        let aprox = *seg.get(3 + n * 2)?;
        let (alto, baixo) = ((aprox >> 4) as u32, (aprox & 15) as u32);
        if inicio > 63 || fim > 63 || inicio > fim || baixo > 13 {
            return None;
        }
        for &c in &nos {
            self.componentes[c].previsto = 0;
        }
        self.fim_de_blocos = 0;
        let mut bits = Bits { dados, pos: 0, acumulado: 0, quantos: 0, marcador: false };
        // Num scan de um componente só, a "MCU" é um bloco; senão, h×v blocos de cada.
        let (mcus_l, mcus_a) = if nos.len() == 1 {
            let c = &self.componentes[nos[0]];
            (c.uteis_l, c.uteis_a)
        } else {
            (self.largura.div_ceil(8 * self.hmax), self.altura.div_ceil(8 * self.vmax))
        };
        let mut contador = 0;
        for my in 0..mcus_a {
            for mx in 0..mcus_l {
                if self.reinicio > 0 && contador > 0 && contador % self.reinicio == 0 {
                    bits.reiniciar();
                    for &c in &nos {
                        self.componentes[c].previsto = 0;
                    }
                    self.fim_de_blocos = 0;
                }
                contador += 1;
                for &c in &nos {
                    let (h, v) = if nos.len() == 1 { (1, 1) } else { (self.componentes[c].h, self.componentes[c].v) };
                    for by in 0..v {
                        for bx in 0..h {
                            let (x, y) = (mx * h + bx, my * v + by);
                            self.bloco(&mut bits, c, x, y, inicio, fim, alto, baixo)?;
                        }
                    }
                }
            }
        }
        // O próximo marcador (que não seja um RST nem um 0xFF 0x00).
        let mut p = bits.pos;
        while p + 1 < dados.len() && !(dados[p] == 0xFF && dados[p + 1] != 0 && !(0xD0..=0xD7).contains(&dados[p + 1])) {
            p += 1;
        }
        Some(p)
    }

    #[allow(clippy::too_many_arguments)]
    fn bloco(&mut self, bits: &mut Bits, c: usize, x: usize, y: usize, inicio: usize, fim: usize, alto: u32, baixo: u32) -> Option<()> {
        let comp = &mut self.componentes[c];
        if x >= comp.blocos_l || y >= comp.blocos_a {
            return Some(());
        }
        let base = (y * comp.blocos_l + x) * 64;
        let (dc, ac) = (&self.dc[comp.dc], &self.ac[comp.ac]);
        let coef = &mut comp.coeficientes[base..base + 64];
        if !self.progressivo {
            // Baseline: o bloco inteiro.
            let s = bits.simbolo(dc)? as u32;
            if s > 11 {
                return None; // a diferença da média tem no máximo 11 bits
            }
            comp.previsto += bits.valor(s);
            coef[0] = comp.previsto as i16;
            let mut k = 1;
            while k < 64 {
                let rs = bits.simbolo(ac)?;
                let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
                if s == 0 {
                    if r != 15 {
                        break; // fim do bloco: o resto é zero
                    }
                    k += 16;
                    continue;
                }
                k += r;
                if k > 63 {
                    break;
                }
                coef[ZIGUE[k]] = bits.valor(s) as i16;
                k += 1;
            }
            return Some(());
        }
        if inicio == 0 {
            // A média (DC): primeiro o valor, depois um bit de cada vez.
            if alto == 0 {
                let s = bits.simbolo(dc)? as u32;
                if s > 11 {
                    return None;
                }
                comp.previsto += bits.valor(s);
                coef[0] = (comp.previsto << baixo) as i16;
            } else if bits.bit() {
                coef[0] |= 1 << baixo;
            }
            return Some(());
        }
        let (p1, m1) = (1i16 << baixo, (-1i16) << baixo);
        let mut k = inicio;
        if alto == 0 {
            // AC, primeira passada.
            if self.fim_de_blocos > 0 {
                self.fim_de_blocos -= 1;
                return Some(());
            }
            while k <= fim {
                let rs = bits.simbolo(ac)?;
                let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
                if s == 0 {
                    if r < 15 {
                        self.fim_de_blocos = (1 << r) - 1 + bits.ler(r);
                        break;
                    }
                    k += 16;
                    continue;
                }
                k += r as usize;
                if k > 63 {
                    break;
                }
                coef[ZIGUE[k]] = (bits.valor(s) * (1 << baixo)) as i16;
                k += 1;
            }
            return Some(());
        }
        // AC, refinamento: um bit a mais em cada coeficiente que já não é
        // zero, e os novos (±1) no meio dos zeros. (Como o libjpeg.)
        let refinar = |bits: &mut Bits, v: &mut i16| {
            if bits.bit() && *v & p1 == 0 {
                *v += if *v >= 0 { p1 } else { m1 };
            }
        };
        if self.fim_de_blocos == 0 {
            while k <= fim {
                let rs = bits.simbolo(ac)?;
                let (mut r, s) = ((rs >> 4) as i32, rs & 15);
                let mut novo = 0;
                if s != 0 {
                    novo = if bits.bit() { p1 } else { m1 };
                } else if r != 15 {
                    self.fim_de_blocos = (1 << r) + bits.ler(r as u32);
                    break;
                }
                while k <= fim {
                    let z = ZIGUE[k];
                    if coef[z] != 0 {
                        refinar(bits, &mut coef[z]);
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                if novo != 0 && k <= 63 {
                    coef[ZIGUE[k]] = novo;
                }
                k += 1;
            }
        }
        if self.fim_de_blocos > 0 {
            while k <= fim {
                let z = ZIGUE[k];
                if coef[z] != 0 {
                    refinar(bits, &mut coef[z]);
                }
                k += 1;
            }
            self.fim_de_blocos -= 1;
        }
        Some(())
    }

    /// Coeficientes → pixels: quantização, IDCT e a conversão de cor.
    fn pixels(&self) -> Option<Imagem> {
        let planos: Vec<Vec<u8>> = self.componentes.iter().map(|c| self.plano(c)).collect();
        let (l, a) = (self.largura, self.altura);
        let mut pixels = Vec::with_capacity(l * a);
        // A cor costuma vir com metade da resolução: cada pixel interpola as
        // amostras vizinhas (com os centros alinhados, como o libjpeg).
        let reduzidos: Vec<bool> = self.componentes.iter().map(|c| c.h != self.hmax || c.v != self.vmax).collect();
        let amostra = |n: usize, x: usize, y: usize| -> i32 {
            let c = &self.componentes[n];
            let plano = &planos[n];
            let largura = c.blocos_l * 8;
            if !reduzidos[n] {
                return plano[y * largura + x] as i32;
            }
            // Posição na grade do componente, em dezesseis avos.
            let posicao = |p: usize, f: usize, fmax: usize, limite: usize| -> (usize, usize, i32) {
                let p16 = (((2 * p + 1) * f * 16) / (2 * fmax)) as i32 - 8;
                let i = (p16.max(0) >> 4) as usize;
                let fracao = if p16 < 0 { 0 } else { p16 & 15 };
                (i.min(limite - 1), (i + 1).min(limite - 1), fracao)
            };
            let (x0, x1, fx) = posicao(x, c.h, self.hmax, (self.largura * c.h).div_ceil(self.hmax));
            let (y0, y1, fy) = posicao(y, c.v, self.vmax, (self.altura * c.v).div_ceil(self.vmax));
            let p = |x: usize, y: usize| plano[y * largura + x] as i32;
            let cima = p(x0, y0) * (16 - fx) + p(x1, y0) * fx;
            let baixo = p(x0, y1) * (16 - fx) + p(x1, y1) * fx;
            (cima * (16 - fy) + baixo * fy + 128) >> 8
        };
        for y in 0..a {
            for x in 0..l {
                let rgb = if planos.len() == 1 {
                    let g = amostra(0, x, y) as u32;
                    g << 16 | g << 8 | g
                } else if self.adobe_rgb || self.componentes.iter().map(|c| c.id).eq(*b"RGB") {
                    (amostra(0, x, y) as u32) << 16 | (amostra(1, x, y) as u32) << 8 | amostra(2, x, y) as u32
                } else {
                    let (yy, cb, cr) = (amostra(0, x, y) << 16, amostra(1, x, y) - 128, amostra(2, x, y) - 128);
                    let limitar = |v: i32| ((v + 32768) >> 16).clamp(0, 255) as u32;
                    let r = limitar(yy + 91881 * cr);
                    let g = limitar(yy - 22554 * cb - 46802 * cr);
                    let b = limitar(yy + 116130 * cb);
                    r << 16 | g << 8 | b
                };
                pixels.push(0xFF00_0000 | rgb);
            }
        }
        Some(Imagem { largura: l as u32, altura: a as u32, pixels })
    }

    /// Um componente inteiro em pixels (8 bits), bloco a bloco.
    fn plano(&self, c: &Componente) -> Vec<u8> {
        let largura = c.blocos_l * 8;
        let mut plano = vec![0u8; largura * c.blocos_a * 8];
        let q = &self.quantizacao[c.quantizacao];
        for by in 0..c.blocos_a {
            for bx in 0..c.blocos_l {
                let base = (by * c.blocos_l + bx) * 64;
                let mut f = [0i64; 64];
                for k in 0..64 {
                    f[ZIGUE[k]] = c.coeficientes[base + ZIGUE[k]] as i64 * q[k] as i64;
                }
                let bloco = idct(&f);
                for y in 0..8 {
                    let linha = (by * 8 + y) * largura + bx * 8;
                    plano[linha..linha + 8].copy_from_slice(&bloco[y * 8..y * 8 + 8]);
                }
            }
        }
        plano
    }
}

/// A IDCT 8×8 (separável: as linhas, depois as colunas), em ponto fixo.
fn idct(f: &[i64; 64]) -> [u8; 64] {
    let mut saida = [0u8; 64];
    // Só a média (o caso mais comum nas áreas lisas): o bloco é uma cor só.
    if f[1..].iter().all(|&v| v == 0) {
        let v = (((f[0] + 4) >> 3) + 128).clamp(0, 255) as u8;
        return [v; 64];
    }
    let mut meio = [0i64; 64];
    for v in 0..8 {
        for x in 0..8 {
            let mut soma = 0;
            for u in 0..8 {
                soma += COSSENOS[x][u] * f[v * 8 + u];
            }
            meio[v * 8 + x] = soma >> 8; // guarda 4 bits a mais de precisão
        }
    }
    for x in 0..8 {
        for y in 0..8 {
            let mut soma = 0;
            for v in 0..8 {
                soma += COSSENOS[y][v] * meio[v * 8 + x];
            }
            saida[y * 8 + x] = (((soma + (1 << 15)) >> 16) + 128).clamp(0, 255) as u8;
        }
    }
    saida
}
