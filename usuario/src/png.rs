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
