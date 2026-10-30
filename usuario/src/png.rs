//! PNG: o formato de imagem sem perdas mais comum.
//!
//! Um PNG é uma assinatura de 8 bytes e uma sequência de "chunks"
//! (tamanho, tipo de 4 letras, dados, CRC-32):
//!
//! ```text
//!   IHDR  largura, altura, profundidade, tipo de cor...
//!   IDAT  os pixels, linha a linha (cada uma com um byte de "filtro" na
//!         frente), comprimidos com o deflate do zlib
//!   IEND  fim
//! ```
//!
//! Para gravar (a captura de tela), usamos o deflate sem compressão: blocos
//! "stored", de até 64 KiB cada, que qualquer leitor de PNG entende.

use alloc::vec;
use alloc::vec::Vec;

/// O CRC-32 do PNG (o mesmo do zlib e do Ethernet), calculado bit a bit
/// com uma tabela de 256 entradas.
fn crc32(partes: &[&[u8]]) -> u32 {
    let mut tabela = [0u32; 256];
    for (n, t) in tabela.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for parte in partes {
        for &b in *parte {
            crc = tabela[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// O Adler-32, a soma de conferência do zlib.
fn adler32(dados: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for pedaco in dados.chunks(5552) {
        for &x in pedaco {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

fn chunk(saida: &mut Vec<u8>, tipo: &[u8; 4], dados: &[u8]) {
    saida.extend_from_slice(&(dados.len() as u32).to_be_bytes());
    saida.extend_from_slice(tipo);
    saida.extend_from_slice(dados);
    saida.extend_from_slice(&crc32(&[tipo, dados]).to_be_bytes());
}

/// Codifica pixels 0x00RRGGBB num PNG (RGB, 8 bits por canal).
pub fn codificar(pixels: &[u32], largura: u32, altura: u32) -> Vec<u8> {
    // As linhas cruas: um byte de filtro (0 = nenhum) e os pixels.
    let mut cru = Vec::with_capacity((largura * 3 + 1) as usize * altura as usize);
    for linha in pixels.chunks(largura as usize).take(altura as usize) {
        cru.push(0);
        for &p in linha {
            cru.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    // zlib: cabeçalho, blocos "stored" e o Adler-32.
    let mut zlib = Vec::with_capacity(cru.len() + cru.len() / 65535 * 5 + 16);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let pedacos: Vec<&[u8]> = cru.chunks(65535).collect();
    for (i, p) in pedacos.iter().enumerate() {
        zlib.push((i + 1 == pedacos.len()) as u8); // BFINAL no último; BTYPE = 00 (stored)
        zlib.extend_from_slice(&(p.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(p.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(p);
    }
    zlib.extend_from_slice(&adler32(&cru).to_be_bytes());

    let mut png = Vec::from(*b"\x89PNG\r\n\x1a\n");
    let mut cabecalho = Vec::new();
    cabecalho.extend_from_slice(&largura.to_be_bytes());
    cabecalho.extend_from_slice(&altura.to_be_bytes());
    cabecalho.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, RGB, deflate, filtro padrão, sem entrelaçar
    chunk(&mut png, b"IHDR", &cabecalho);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

// -----------------------------------------------------------------------------
// Ler: o inflate (descompressão do deflate) e a decodificação do PNG
// -----------------------------------------------------------------------------

/// Uma imagem decodificada: pixels 0xAARRGGBB (com transparência).
pub struct Imagem {
    pub largura: u32,
    pub altura: u32,
    pub pixels: Vec<u32>,
}

/// Lê os bits do deflate: do menos significativo para o mais, byte a byte.
struct Bits<'a> {
    dados: &'a [u8],
    posicao: usize,
    buffer: u32,
    quantos: u32,
}

impl Bits<'_> {
    fn bits(&mut self, n: u32) -> Option<u32> {
        while self.quantos < n {
            let byte = *self.dados.get(self.posicao)? as u32;
            self.posicao += 1;
            self.buffer |= byte << self.quantos;
            self.quantos += 8;
        }
        let v = self.buffer & ((1u64 << n) - 1) as u32;
        self.buffer >>= n;
        self.quantos -= n;
        Some(v)
    }
}

/// Um código de Huffman canônico: quantos códigos há de cada comprimento e
/// os símbolos em ordem (como no `puff.c` do zlib).
struct Huffman {
    contagem: [u16; 16],
    simbolos: Vec<u16>,
}

impl Huffman {
    fn de_comprimentos(comprimentos: &[u8]) -> Huffman {
        let mut contagem = [0u16; 16];
        for &c in comprimentos {
            contagem[c as usize] += 1;
        }
        contagem[0] = 0;
        let mut inicio = [0u16; 16];
        for i in 1..16 {
            inicio[i] = inicio[i - 1] + contagem[i - 1];
        }
        let mut simbolos = vec![0u16; comprimentos.len()];
        for (s, &c) in comprimentos.iter().enumerate() {
            if c != 0 {
                simbolos[inicio[c as usize] as usize] = s as u16;
                inicio[c as usize] += 1;
            }
        }
        Huffman { contagem, simbolos }
    }

    /// Lê um símbolo, um bit por vez.
    fn ler(&self, b: &mut Bits) -> Option<u16> {
        let (mut codigo, mut primeiro, mut indice) = (0i32, 0i32, 0i32);
        for comprimento in 1..16 {
            codigo |= b.bits(1)? as i32;
            let n = self.contagem[comprimento] as i32;
            if codigo - n < primeiro {
                return self.simbolos.get((indice + codigo - primeiro) as usize).copied();
            }
            indice += n;
            primeiro = (primeiro + n) << 1;
            codigo <<= 1;
        }
        None
    }
}

const BASE_COMPRIMENTO: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const EXTRA_COMPRIMENTO: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const BASE_DISTANCIA: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const EXTRA_DISTANCIA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Os símbolos de um bloco comprimido: literais, ou (comprimento, distância)
/// = "copie N bytes de D bytes atrás".
fn bloco(b: &mut Bits, saida: &mut Vec<u8>, literais: &Huffman, distancias: &Huffman) -> Option<()> {
    loop {
        let s = literais.ler(b)?;
        match s {
            0..=255 => saida.push(s as u8),
            256 => return Some(()),
            _ => {
                let i = (s - 257) as usize;
                let comprimento = *BASE_COMPRIMENTO.get(i)? as usize + b.bits(*EXTRA_COMPRIMENTO.get(i)? as u32)? as usize;
                let d = distancias.ler(b)? as usize;
                let distancia = *BASE_DISTANCIA.get(d)? as usize + b.bits(*EXTRA_DISTANCIA.get(d)? as u32)? as usize;
                if distancia > saida.len() {
                    return None;
                }
                let inicio = saida.len() - distancia;
                for k in 0..comprimento {
                    saida.push(saida[inicio + k]); // pode se sobrepor: é de propósito
                }
            }
        }
    }
}

/// Descomprime um fluxo zlib (cabeçalho de 2 bytes + deflate).
pub fn inflar(zlib: &[u8]) -> Option<Vec<u8>> {
    if zlib.len() < 2 || zlib[0] & 0x0F != 8 {
        return None;
    }
    let mut b = Bits { dados: &zlib[2..], posicao: 0, buffer: 0, quantos: 0 };
    let mut saida = Vec::new();
    loop {
        let ultimo = b.bits(1)?;
        match b.bits(2)? {
            0 => {
                // Sem compressão: alinha no byte, LEN, NLEN e os bytes.
                (b.buffer, b.quantos) = (0, 0);
                let p = b.posicao;
                let n = u16::from_le_bytes([*b.dados.get(p)?, *b.dados.get(p + 1)?]) as usize;
                saida.extend_from_slice(b.dados.get(p + 4..p + 4 + n)?);
                b.posicao = p + 4 + n;
            }
            1 => {
                // Huffman fixo (os comprimentos que o padrão define).
                let mut c = [0u8; 288];
                c[..144].fill(8);
                c[144..256].fill(9);
                c[256..280].fill(7);
                c[280..].fill(8);
                bloco(&mut b, &mut saida, &Huffman::de_comprimentos(&c), &Huffman::de_comprimentos(&[5; 30]))?;
            }
            2 => {
                // Huffman dinâmico: os comprimentos vêm comprimidos por outro Huffman.
                let nlit = b.bits(5)? as usize + 257;
                let ndist = b.bits(5)? as usize + 1;
                let ncod = b.bits(4)? as usize + 4;
                const ORDEM: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut c = [0u8; 19];
                for &i in ORDEM.iter().take(ncod) {
                    c[i] = b.bits(3)? as u8;
                }
                let codigos = Huffman::de_comprimentos(&c);
                let mut comprimentos = Vec::with_capacity(nlit + ndist);
                while comprimentos.len() < nlit + ndist {
                    match codigos.ler(&mut b)? {
                        s @ 0..=15 => comprimentos.push(s as u8),
                        16 => {
                            let anterior = *comprimentos.last()?;
                            for _ in 0..3 + b.bits(2)? {
                                comprimentos.push(anterior);
                            }
                        }
                        17 => comprimentos.resize(comprimentos.len() + 3 + b.bits(3)? as usize, 0),
                        _ => comprimentos.resize(comprimentos.len() + 11 + b.bits(7)? as usize, 0),
                    }
                }
                let literais = Huffman::de_comprimentos(&comprimentos[..nlit]);
                let distancias = Huffman::de_comprimentos(&comprimentos[nlit..nlit + ndist]);
                bloco(&mut b, &mut saida, &literais, &distancias)?;
            }
            _ => return None,
        }
        if ultimo == 1 {
            return Some(saida);
        }
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Desfaz os filtros de uma (sub)imagem: cada linha diz como foi codificada
/// em relação à esquerda (a), à de cima (b) e à de cima-esquerda (c).
fn desfiltrar(dados: &[u8], linhas: usize, por_linha: usize, bpp: usize) -> Option<Vec<u8>> {
    let mut saida = vec![0u8; linhas * por_linha];
    for y in 0..linhas {
        let entrada = dados.get(y * (por_linha + 1)..(y + 1) * (por_linha + 1))?;
        let filtro = entrada[0];
        for x in 0..por_linha {
            let a = if x >= bpp { saida[y * por_linha + x - bpp] } else { 0 };
            let b = if y > 0 { saida[(y - 1) * por_linha + x] } else { 0 };
            let c = if x >= bpp && y > 0 { saida[(y - 1) * por_linha + x - bpp] } else { 0 };
            let v = entrada[1 + x];
            saida[y * por_linha + x] = match filtro {
                0 => v,
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(b),
                3 => v.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => v.wrapping_add(paeth(a, b, c)),
                _ => return None,
            };
        }
    }
    Some(saida)
}

/// Lê um PNG: todos os tipos de cor e profundidades, e o entrelaçamento Adam7.
pub fn decodificar(png: &[u8]) -> Option<Imagem> {
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let (mut largura, mut altura, mut profundidade, mut cor, mut entrelacado) = (0u32, 0u32, 0u8, 0u8, false);
    let mut paleta: Vec<u32> = Vec::new();
    let mut transparente: Option<u16> = None; // cinza/RGB: a cor que vale como transparente (tRNS)
    let mut transparente_rgb: Option<(u16, u16, u16)> = None;
    let mut idat = Vec::new();
    let mut p = 8;
    while p + 8 <= png.len() {
        let n = u32::from_be_bytes(png[p..p + 4].try_into().ok()?) as usize;
        let tipo = &png[p + 4..p + 8];
        let d = png.get(p + 8..p + 8 + n)?;
        match tipo {
            b"IHDR" => {
                largura = u32::from_be_bytes(d[0..4].try_into().ok()?);
                altura = u32::from_be_bytes(d[4..8].try_into().ok()?);
                (profundidade, cor, entrelacado) = (d[8], d[9], d[12] == 1);
            }
            b"PLTE" => paleta = d.chunks_exact(3).map(|c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect(),
            b"tRNS" => match cor {
                3 => {
                    for (i, &a) in d.iter().enumerate() {
                        if let Some(c) = paleta.get_mut(i) {
                            *c = (*c & 0xFFFFFF) | (a as u32) << 24;
                        }
                    }
                }
                0 if d.len() >= 2 => transparente = Some(u16::from_be_bytes([d[0], d[1]])),
                2 if d.len() >= 6 => {
                    let v = |i: usize| u16::from_be_bytes([d[i], d[i + 1]]);
                    transparente_rgb = Some((v(0), v(2), v(4)));
                }
                _ => {}
            },
            b"IDAT" => idat.extend_from_slice(d),
            b"IEND" => break,
            _ => {}
        }
        p += 12 + n;
    }
    if largura == 0 || altura == 0 || largura > 16384 || altura > 16384 {
        return None;
    }
    let canais = match cor {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let bits_por_pixel = canais * profundidade as usize;
    let bpp = bits_por_pixel.div_ceil(8);
    let cru = inflar(&idat)?;
    let mut pixels = vec![0u32; (largura * altura) as usize];

    // Um pixel (x, y) de uma linha desfiltrada.
    let converter = |linha: &[u8], x: usize| -> u32 {
        let amostra = |i: usize| -> u16 {
            match profundidade {
                16 => u16::from_be_bytes([linha[(x * canais + i) * 2], linha[(x * canais + i) * 2 + 1]]),
                8 => linha[x * canais + i] as u16,
                _ => {
                    let bit = x * profundidade as usize;
                    let v = (linha[bit / 8] >> (8 - profundidade as usize - bit % 8)) & ((1 << profundidade) - 1);
                    v as u16
                }
            }
        };
        let oito = |v: u16| -> u32 {
            match profundidade {
                16 => (v >> 8) as u32,
                8 => v as u32,
                d => (v as u32 * 255) / ((1u32 << d) - 1),
            }
        };
        match cor {
            3 => paleta.get(amostra(0) as usize).copied().unwrap_or(0xFF000000),
            0 => {
                let v = amostra(0);
                let g = oito(v);
                let a = if transparente == Some(v) { 0 } else { 0xFF };
                a << 24 | g << 16 | g << 8 | g
            }
            4 => {
                let g = oito(amostra(0));
                oito(amostra(1)) << 24 | g << 16 | g << 8 | g
            }
            2 => {
                let (r, g, b) = (amostra(0), amostra(1), amostra(2));
                let a = if transparente_rgb == Some((r, g, b)) { 0 } else { 0xFF };
                a << 24 | oito(r) << 16 | oito(g) << 8 | oito(b)
            }
            _ => oito(amostra(3)) << 24 | oito(amostra(0)) << 16 | oito(amostra(1)) << 8 | oito(amostra(2)),
        }
    };

    // Adam7: 7 passadas, cada uma uma subimagem com a sua grade.
    const PASSADAS: [(u32, u32, u32, u32); 7] = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];
    let passadas: &[(u32, u32, u32, u32)] = if entrelacado { &PASSADAS } else { &[(0, 0, 1, 1)] };
    let mut onde = 0;
    for &(x0, y0, dx, dy) in passadas {
        let (w, h) = ((largura.saturating_sub(x0)).div_ceil(dx), (altura.saturating_sub(y0)).div_ceil(dy));
        if w == 0 || h == 0 {
            continue;
        }
        let por_linha = (w as usize * bits_por_pixel).div_ceil(8);
        let tamanho = h as usize * (por_linha + 1);
        let sub = desfiltrar(cru.get(onde..onde + tamanho)?, h as usize, por_linha, bpp)?;
        onde += tamanho;
        for y in 0..h as usize {
            let linha = &sub[y * por_linha..(y + 1) * por_linha];
            for x in 0..w as usize {
                let (px, py) = (x0 + x as u32 * dx, y0 + y as u32 * dy);
                pixels[(py * largura + px) as usize] = converter(linha, x);
            }
        }
    }
    Some(Imagem { largura, altura, pixels })
}
