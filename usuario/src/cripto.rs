//! Criptografia para o TLS 1.3: HMAC e HKDF (sobre o SHA-256), X25519 (a
//! troca de chaves), ChaCha20-Poly1305 e AES-128-GCM (as cifras).
//!
//! Tudo conferido com os vetores de teste das RFCs (veja o teste
//! "criptografia" em bin/teste.rs). Feito para ser lido, não para resistir a
//! ataques de tempo: o AES, por exemplo, usa uma tabela (a S-box) indexada
//! pela chave, o que vaza informação pelo cache para quem roda na mesma
//! máquina. Um TLS de produção usaria código de tempo constante.

use crate::sha256::{sha256, Sha256};
use alloc::vec::Vec;

// -----------------------------------------------------------------------------
// HMAC (RFC 2104) e HKDF (RFC 5869)
// -----------------------------------------------------------------------------

/// Um "hash com chave": só quem tem a chave produz (ou confere) o resultado.
pub fn hmac_sha256(chave: &[u8], partes: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if chave.len() > 64 {
        k[..32].copy_from_slice(&sha256(chave));
    } else {
        k[..chave.len()].copy_from_slice(chave);
    }
    let mut dentro = Sha256::novo();
    dentro.atualizar(&k.map(|b| b ^ 0x36));
    for p in partes {
        dentro.atualizar(p);
    }
    let mut fora = Sha256::novo();
    fora.atualizar(&k.map(|b| b ^ 0x5c));
    fora.atualizar(&dentro.terminar());
    fora.terminar()
}

/// HKDF, primeira metade: concentra a entropia de `material` numa chave.
pub fn hkdf_extrair(sal: &[u8], material: &[u8]) -> [u8; 32] {
    hmac_sha256(sal, &[material])
}

/// HKDF, segunda metade: estica uma chave em `tamanho` bytes, para `info`.
pub fn hkdf_expandir(prk: &[u8], info: &[u8], tamanho: usize) -> Vec<u8> {
    let mut saida = Vec::with_capacity(tamanho);
    let mut anterior: Vec<u8> = Vec::new();
    let mut i = 1u8;
    while saida.len() < tamanho {
        let t = hmac_sha256(prk, &[&anterior, info, &[i]]);
        saida.extend_from_slice(&t);
        anterior = t.to_vec();
        i += 1;
    }
    saida.truncate(tamanho);
    saida
}

/// O HKDF-Expand-Label do TLS 1.3: a "info" leva o tamanho, o rótulo
/// (com "tls13 " na frente) e um contexto (em geral, o hash da conversa).
pub fn expandir_rotulo(segredo: &[u8], rotulo: &str, contexto: &[u8], tamanho: usize) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(&(tamanho as u16).to_be_bytes());
    info.push(6 + rotulo.len() as u8);
    info.extend_from_slice(b"tls13 ");
    info.extend_from_slice(rotulo.as_bytes());
    info.push(contexto.len() as u8);
    info.extend_from_slice(contexto);
    hkdf_expandir(segredo, &info, tamanho)
}

// -----------------------------------------------------------------------------
// X25519 (RFC 7748): a troca de chaves de Diffie-Hellman na curva 25519
// -----------------------------------------------------------------------------
//
// Cada lado escolhe um número secreto `a` e publica a·P (P, um ponto fixo da
// curva). Com o público do outro, b·P, cada um calcula a·(b·P) = b·(a·P): o
// mesmo segredo, sem que ele tenha passado pela rede. As contas são com
// números de 255 bits, módulo o primo 2^255 − 19, guardados em 5 "membros"
// de 51 bits (assim o produto de dois membros cabe em 128 bits).

type Fe = [u64; 5];
const M51: u64 = (1 << 51) - 1;

fn fe_ler(b: &[u8; 32]) -> Fe {
    let ler = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    [ler(0) & M51, (ler(6) >> 3) & M51, (ler(12) >> 6) & M51, (ler(19) >> 1) & M51, (ler(24) >> 12) & M51]
}

fn fe_carregar(t: &mut [u64; 5]) {
    t[1] += t[0] >> 51;
    t[0] &= M51;
    t[2] += t[1] >> 51;
    t[1] &= M51;
    t[3] += t[2] >> 51;
    t[2] &= M51;
    t[4] += t[3] >> 51;
    t[3] &= M51;
    t[0] += 19 * (t[4] >> 51);
    t[4] &= M51;
}

/// Reduz de vez (o resultado fica entre 0 e p − 1) e escreve em bytes.
fn fe_escrever(h: &Fe) -> [u8; 32] {
    let mut t = *h;
    fe_carregar(&mut t);
    fe_carregar(&mut t);
    // Somar 19 e depois 2^255 − 19 (que some no fim) decide, sem desvios,
    // se é preciso subtrair p.
    t[0] += 19;
    fe_carregar(&mut t);
    t[0] += (1 << 51) - 19;
    for x in &mut t[1..] {
        *x += (1 << 51) - 1;
    }
    t[1] += t[0] >> 51;
    t[0] &= M51;
    t[2] += t[1] >> 51;
    t[1] &= M51;
    t[3] += t[2] >> 51;
    t[2] &= M51;
    t[4] += t[3] >> 51;
    t[3] &= M51;
    t[4] &= M51;
    let mut b = [0u8; 32];
    b[0..8].copy_from_slice(&(t[0] | t[1] << 51).to_le_bytes());
    b[8..16].copy_from_slice(&(t[1] >> 13 | t[2] << 38).to_le_bytes());
    b[16..24].copy_from_slice(&(t[2] >> 26 | t[3] << 25).to_le_bytes());
    b[24..32].copy_from_slice(&(t[3] >> 39 | t[4] << 12).to_le_bytes());
    b
}

fn fe_somar(a: &Fe, b: &Fe) -> Fe {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3], a[4] + b[4]]
}

/// a − b, somando 2p antes para nada ficar negativo.
fn fe_subtrair(a: &Fe, b: &Fe) -> Fe {
    let mut r = [
        a[0] + 0xFFFFFFFFFFFDA - b[0],
        a[1] + 0xFFFFFFFFFFFFE - b[1],
        a[2] + 0xFFFFFFFFFFFFE - b[2],
        a[3] + 0xFFFFFFFFFFFFE - b[3],
        a[4] + 0xFFFFFFFFFFFFE - b[4],
    ];
    fe_carregar(&mut r);
    r
}

fn fe_multiplicar(a: &Fe, b: &Fe) -> Fe {
    let m = |x: u64, y: u64| x as u128 * y as u128;
    // 2^255 = 19 (mód p): o que passa do quinto membro volta multiplicado por 19.
    let (b1, b2, b3, b4) = (b[1] * 19, b[2] * 19, b[3] * 19, b[4] * 19);
    let r0 = m(a[0], b[0]) + m(a[4], b1) + m(a[3], b2) + m(a[2], b3) + m(a[1], b4);
    let r1 = m(a[0], b[1]) + m(a[1], b[0]) + m(a[4], b2) + m(a[3], b3) + m(a[2], b4);
    let r2 = m(a[0], b[2]) + m(a[1], b[1]) + m(a[2], b[0]) + m(a[4], b3) + m(a[3], b4);
    let r3 = m(a[0], b[3]) + m(a[1], b[2]) + m(a[2], b[1]) + m(a[3], b[0]) + m(a[4], b4);
    let r4 = m(a[0], b[4]) + m(a[1], b[3]) + m(a[2], b[2]) + m(a[3], b[1]) + m(a[4], b[0]);
    let mut r = [0u64; 5];
    let mut vai = r0 >> 51;
    r[0] = r0 as u64 & M51;
    let r1 = r1 + vai;
    vai = r1 >> 51;
    r[1] = r1 as u64 & M51;
    let r2 = r2 + vai;
    vai = r2 >> 51;
    r[2] = r2 as u64 & M51;
    let r3 = r3 + vai;
    vai = r3 >> 51;
    r[3] = r3 as u64 & M51;
    let r4 = r4 + vai;
    vai = r4 >> 51;
    r[4] = r4 as u64 & M51;
    r[0] += vai as u64 * 19;
    r[1] += r[0] >> 51;
    r[0] &= M51;
    r
}

/// z^(p−2) = 1/z (pelo pequeno teorema de Fermat).
fn fe_inverter(z: &Fe) -> Fe {
    // p − 2 = 2^255 − 21: em binário, 250 uns, depois 01011.
    let mut r: Fe = [1, 0, 0, 0, 0];
    for i in (0..255).rev() {
        r = fe_multiplicar(&r, &r);
        let bit = if i >= 5 { 1 } else { (0b01011 >> i) & 1 };
        if bit == 1 {
            r = fe_multiplicar(&r, z);
        }
    }
    r
}

fn fe_trocar(a: &mut Fe, b: &mut Fe, trocar: u64) {
    let mascara = 0u64.wrapping_sub(trocar);
    for i in 0..5 {
        let x = mascara & (a[i] ^ b[i]);
        a[i] ^= x;
        b[i] ^= x;
    }
}

/// k·u: o escalar `k` (o segredo) vezes o ponto de coordenada `u`.
pub fn x25519(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut k = *k;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    let x1 = fe_ler(u);
    let (mut x2, mut z2, mut x3, mut z3): (Fe, Fe, Fe, Fe) = ([1, 0, 0, 0, 0], [0; 5], x1, [1, 0, 0, 0, 0]);
    let a24: Fe = [121665, 0, 0, 0, 0];
    let mut trocado = 0u64;
    // A "escada de Montgomery": a mesma sequência de contas para cada bit,
    // seja ele 0 ou 1 (só a troca muda).
    for t in (0..255).rev() {
        let bit = (k[t / 8] >> (t % 8)) as u64 & 1;
        trocado ^= bit;
        fe_trocar(&mut x2, &mut x3, trocado);
        fe_trocar(&mut z2, &mut z3, trocado);
        trocado = bit;
        let a = fe_somar(&x2, &z2);
        let aa = fe_multiplicar(&a, &a);
        let b = fe_subtrair(&x2, &z2);
        let bb = fe_multiplicar(&b, &b);
        let e = fe_subtrair(&aa, &bb);
        let c = fe_somar(&x3, &z3);
        let d = fe_subtrair(&x3, &z3);
        let da = fe_multiplicar(&d, &a);
        let cb = fe_multiplicar(&c, &b);
        let soma = fe_somar(&da, &cb);
        x3 = fe_multiplicar(&soma, &soma);
        let diferenca = fe_subtrair(&da, &cb);
        z3 = fe_multiplicar(&x1, &fe_multiplicar(&diferenca, &diferenca));
        x2 = fe_multiplicar(&aa, &bb);
        z2 = fe_multiplicar(&e, &fe_somar(&aa, &fe_multiplicar(&a24, &e)));
    }
    fe_trocar(&mut x2, &mut x3, trocado);
    fe_trocar(&mut z2, &mut z3, trocado);
    fe_escrever(&fe_multiplicar(&x2, &fe_inverter(&z2)))
}

/// A chave pública que vai com o segredo `k` (k vezes o ponto de u = 9).
pub fn x25519_publica(k: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(k, &base)
}

// -----------------------------------------------------------------------------
// ChaCha20 e Poly1305 (RFC 8439)
// -----------------------------------------------------------------------------

fn chacha20_bloco(chave: &[u8; 32], contador: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut s = [0u32; 16];
    s[..4].copy_from_slice(&[0x61707865, 0x3320646e, 0x79622d32, 0x6b206574]); // "expand 32-byte k"
    for i in 0..8 {
        s[4 + i] = u32::from_le_bytes(chave[i * 4..i * 4 + 4].try_into().unwrap());
    }
    s[12] = contador;
    for i in 0..3 {
        s[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let mut x = s;
    let quarto = |x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize| {
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(16);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(12);
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(8);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(7);
    };
    for _ in 0..10 {
        quarto(&mut x, 0, 4, 8, 12);
        quarto(&mut x, 1, 5, 9, 13);
        quarto(&mut x, 2, 6, 10, 14);
        quarto(&mut x, 3, 7, 11, 15);
        quarto(&mut x, 0, 5, 10, 15);
        quarto(&mut x, 1, 6, 11, 12);
        quarto(&mut x, 2, 7, 8, 13);
        quarto(&mut x, 3, 4, 9, 14);
    }
    let mut saida = [0u8; 64];
    for i in 0..16 {
        saida[i * 4..i * 4 + 4].copy_from_slice(&x[i].wrapping_add(s[i]).to_le_bytes());
    }
    saida
}

/// Cifra (ou decifra: é a mesma conta) com o fluxo do ChaCha20.
pub fn chacha20(chave: &[u8; 32], contador: u32, nonce: &[u8; 12], dados: &mut [u8]) {
    for (i, pedaco) in dados.chunks_mut(64).enumerate() {
        let fluxo = chacha20_bloco(chave, contador.wrapping_add(i as u32), nonce);
        for (b, f) in pedaco.iter_mut().zip(fluxo) {
            *b ^= f;
        }
    }
}

/// O autenticador Poly1305: um polinômio avaliado num ponto secreto, módulo
/// 2^130 − 5 (membros de 26 bits).
pub fn poly1305(chave: &[u8; 32], mensagem: &[u8]) -> [u8; 16] {
    let le = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let r = [
        le(chave, 0) & 0x3ffffff,
        (le(chave, 3) >> 2) & 0x3ffff03,
        (le(chave, 6) >> 4) & 0x3ffc0ff,
        (le(chave, 9) >> 6) & 0x3f03fff,
        (le(chave, 12) >> 8) & 0x00fffff,
    ];
    let s = [r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
    let mut h = [0u32; 5];
    for pedaco in mensagem.chunks(16) {
        let mut bloco = [0u8; 17];
        bloco[..pedaco.len()].copy_from_slice(pedaco);
        bloco[pedaco.len()] = 1; // o bit acima do último byte
        let alto = (bloco[16] as u32) << 24;
        h[0] += le(&bloco, 0) & 0x3ffffff;
        h[1] += (le(&bloco, 3) >> 2) & 0x3ffffff;
        h[2] += (le(&bloco, 6) >> 4) & 0x3ffffff;
        h[3] += (le(&bloco, 9) >> 6) & 0x3ffffff;
        h[4] += (le(&bloco, 12) >> 8) | alto;
        let m = |a: u32, b: u32| a as u64 * b as u64;
        let d0 = m(h[0], r[0]) + m(h[1], s[3]) + m(h[2], s[2]) + m(h[3], s[1]) + m(h[4], s[0]);
        let mut d1 = m(h[0], r[1]) + m(h[1], r[0]) + m(h[2], s[3]) + m(h[3], s[2]) + m(h[4], s[1]);
        let mut d2 = m(h[0], r[2]) + m(h[1], r[1]) + m(h[2], r[0]) + m(h[3], s[3]) + m(h[4], s[2]);
        let mut d3 = m(h[0], r[3]) + m(h[1], r[2]) + m(h[2], r[1]) + m(h[3], r[0]) + m(h[4], s[3]);
        let mut d4 = m(h[0], r[4]) + m(h[1], r[3]) + m(h[2], r[2]) + m(h[3], r[1]) + m(h[4], r[0]);
        h[0] = d0 as u32 & 0x3ffffff;
        d1 += d0 >> 26;
        h[1] = d1 as u32 & 0x3ffffff;
        d2 += d1 >> 26;
        h[2] = d2 as u32 & 0x3ffffff;
        d3 += d2 >> 26;
        h[3] = d3 as u32 & 0x3ffffff;
        d4 += d3 >> 26;
        h[4] = d4 as u32 & 0x3ffffff;
        h[0] += (d4 >> 26) as u32 * 5;
        h[1] += h[0] >> 26;
        h[0] &= 0x3ffffff;
    }
    // Reduz de vez e escolhe h ou h − p (sem desvios).
    let mut c;
    c = h[1] >> 26;
    h[1] &= 0x3ffffff;
    h[2] += c;
    c = h[2] >> 26;
    h[2] &= 0x3ffffff;
    h[3] += c;
    c = h[3] >> 26;
    h[3] &= 0x3ffffff;
    h[4] += c;
    c = h[4] >> 26;
    h[4] &= 0x3ffffff;
    h[0] += c * 5;
    c = h[0] >> 26;
    h[0] &= 0x3ffffff;
    h[1] += c;
    let mut g = [0u32; 5];
    g[0] = h[0] + 5;
    c = g[0] >> 26;
    g[0] &= 0x3ffffff;
    g[1] = h[1] + c;
    c = g[1] >> 26;
    g[1] &= 0x3ffffff;
    g[2] = h[2] + c;
    c = g[2] >> 26;
    g[2] &= 0x3ffffff;
    g[3] = h[3] + c;
    c = g[3] >> 26;
    g[3] &= 0x3ffffff;
    g[4] = (h[4] + c).wrapping_sub(1 << 26);
    let mascara = (g[4] >> 31).wrapping_sub(1); // tudo 1 se g ≥ 0 (h ≥ p)
    for i in 0..5 {
        h[i] = (h[i] & !mascara) | (g[i] & mascara);
    }
    let h0 = h[0] | h[1] << 26;
    let h1 = h[1] >> 6 | h[2] << 20;
    let h2 = h[2] >> 12 | h[3] << 14;
    let h3 = h[3] >> 18 | h[4] << 8;
    let mut f = h0 as u64 + le(chave, 16) as u64;
    let mut t = [0u8; 16];
    t[0..4].copy_from_slice(&(f as u32).to_le_bytes());
    f = h1 as u64 + le(chave, 20) as u64 + (f >> 32);
    t[4..8].copy_from_slice(&(f as u32).to_le_bytes());
    f = h2 as u64 + le(chave, 24) as u64 + (f >> 32);
    t[8..12].copy_from_slice(&(f as u32).to_le_bytes());
    f = h3 as u64 + le(chave, 28) as u64 + (f >> 32);
    t[12..16].copy_from_slice(&(f as u32).to_le_bytes());
    t
}

fn poly1305_aead(chave_otk: &[u8; 32], extra: &[u8], cifrado: &[u8]) -> [u8; 16] {
    let mut m = Vec::with_capacity(extra.len() + cifrado.len() + 48);
    m.extend_from_slice(extra);
    m.resize(m.len().div_ceil(16) * 16, 0);
    m.extend_from_slice(cifrado);
    m.resize(m.len().div_ceil(16) * 16, 0);
    m.extend_from_slice(&(extra.len() as u64).to_le_bytes());
    m.extend_from_slice(&(cifrado.len() as u64).to_le_bytes());
    poly1305(chave_otk, &m)
}

/// Cifra `dados` e devolve cifrado + etiqueta (16 bytes) que autentica também `extra`.
pub fn chacha20_poly1305_cifrar(chave: &[u8; 32], nonce: &[u8; 12], extra: &[u8], dados: &[u8]) -> Vec<u8> {
    let otk: [u8; 32] = chacha20_bloco(chave, 0, nonce)[..32].try_into().unwrap();
    let mut saida = dados.to_vec();
    chacha20(chave, 1, nonce, &mut saida);
    let etiqueta = poly1305_aead(&otk, extra, &saida);
    saida.extend_from_slice(&etiqueta);
    saida
}

/// Confere a etiqueta e decifra (None: os dados foram alterados).
pub fn chacha20_poly1305_decifrar(chave: &[u8; 32], nonce: &[u8; 12], extra: &[u8], cifrado: &[u8]) -> Option<Vec<u8>> {
    let n = cifrado.len().checked_sub(16)?;
    let otk: [u8; 32] = chacha20_bloco(chave, 0, nonce)[..32].try_into().unwrap();
    let etiqueta = poly1305_aead(&otk, extra, &cifrado[..n]);
    if !iguais(&etiqueta, &cifrado[n..]) {
        return None;
    }
    let mut saida = cifrado[..n].to_vec();
    chacha20(chave, 1, nonce, &mut saida);
    Some(saida)
}

/// Compara sem parar no primeiro byte diferente (não vaza onde diferem).
pub fn iguais(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

// -----------------------------------------------------------------------------
// AES-128 (FIPS 197) e o modo GCM (NIST SP 800-38D)
// -----------------------------------------------------------------------------

/// A S-box do AES, calculada: o inverso no corpo GF(2^8) seguido de uma
/// transformação afim (é de onde vêm os números da tabela das referências).
fn caixa_s() -> [u8; 256] {
    let vezes = |mut a: u8, mut b: u8| {
        let mut p = 0u8;
        while b != 0 {
            if b & 1 != 0 {
                p ^= a;
            }
            let alto = a & 0x80;
            a <<= 1;
            if alto != 0 {
                a ^= 0x1b;
            }
            b >>= 1;
        }
        p
    };
    let mut s = [0u8; 256];
    for (x, saida) in s.iter_mut().enumerate() {
        // x^254 = x^(−1) (e 0 vai para 0)
        let mut inverso = 1u8;
        for _ in 0..254 {
            inverso = vezes(inverso, x as u8);
        }
        if x == 0 {
            inverso = 0;
        }
        let b = inverso;
        *saida = b ^ b.rotate_left(1) ^ b.rotate_left(2) ^ b.rotate_left(3) ^ b.rotate_left(4) ^ 0x63;
    }
    s
}

pub struct Aes128 {
    rodadas: [[u8; 16]; 11],
    s: [u8; 256],
    /// H = AES(0): a chave do GHASH.
    h: u128,
}

fn xtime(b: u8) -> u8 {
    (b << 1) ^ if b & 0x80 != 0 { 0x1b } else { 0 }
}

impl Aes128 {
    pub fn novo(chave: &[u8; 16]) -> Aes128 {
        let s = caixa_s();
        let mut rodadas = [[0u8; 16]; 11];
        rodadas[0] = *chave;
        let mut rcon = 1u8;
        for r in 1..11 {
            let anterior = rodadas[r - 1];
            let mut t = [anterior[13], anterior[14], anterior[15], anterior[12]];
            for b in &mut t {
                *b = s[*b as usize];
            }
            t[0] ^= rcon;
            rcon = xtime(rcon);
            for i in 0..16 {
                let base = if i < 4 { t[i] } else { rodadas[r][i - 4] };
                rodadas[r][i] = anterior[i] ^ base;
            }
        }
        let mut aes = Aes128 { rodadas, s, h: 0 };
        aes.h = u128::from_be_bytes(aes.cifrar_bloco(&[0; 16]));
        aes
    }

    pub fn cifrar_bloco(&self, entrada: &[u8; 16]) -> [u8; 16] {
        let mut e = *entrada;
        for (b, k) in e.iter_mut().zip(&self.rodadas[0]) {
            *b ^= k;
        }
        for r in 1..11 {
            // SubBytes e ShiftRows (a linha i anda i colunas para a esquerda)
            let mut t = [0u8; 16];
            for c in 0..4 {
                for l in 0..4 {
                    t[c * 4 + l] = self.s[e[((c + l) % 4) * 4 + l] as usize];
                }
            }
            // MixColumns (menos na última rodada)
            if r < 10 {
                for c in 0..4 {
                    let col = [t[c * 4], t[c * 4 + 1], t[c * 4 + 2], t[c * 4 + 3]];
                    let todos = col[0] ^ col[1] ^ col[2] ^ col[3];
                    for l in 0..4 {
                        t[c * 4 + l] = col[l] ^ todos ^ xtime(col[l] ^ col[(l + 1) % 4]);
                    }
                }
            }
            for (b, (x, k)) in e.iter_mut().zip(t.iter().zip(&self.rodadas[r])) {
                *b = x ^ k;
            }
        }
        e
    }

    /// Multiplicação no corpo GF(2^128) do GCM (com os bits "ao contrário").
    fn vezes_h(&self, x: u128) -> u128 {
        let mut z = 0u128;
        let mut v = self.h;
        for i in 0..128 {
            if (x >> (127 - i)) & 1 == 1 {
                z ^= v;
            }
            v = if v & 1 == 1 { (v >> 1) ^ (0xE1 << 120) } else { v >> 1 };
        }
        z
    }

    fn ghash(&self, extra: &[u8], cifrado: &[u8]) -> u128 {
        let mut x = 0u128;
        for dados in [extra, cifrado] {
            for pedaco in dados.chunks(16) {
                let mut bloco = [0u8; 16];
                bloco[..pedaco.len()].copy_from_slice(pedaco);
                x = self.vezes_h(x ^ u128::from_be_bytes(bloco));
            }
        }
        let tamanhos = ((extra.len() as u128 * 8) << 64) | cifrado.len() as u128 * 8;
        self.vezes_h(x ^ tamanhos)
    }

    /// O modo contador: cifra o contador e mistura com os dados.
    fn contador(&self, nonce: &[u8; 12], dados: &mut [u8]) {
        for (i, pedaco) in dados.chunks_mut(16).enumerate() {
            let mut bloco = [0u8; 16];
            bloco[..12].copy_from_slice(nonce);
            bloco[12..].copy_from_slice(&(i as u32 + 2).to_be_bytes()); // 1 é o da etiqueta
            let fluxo = self.cifrar_bloco(&bloco);
            for (b, f) in pedaco.iter_mut().zip(fluxo) {
                *b ^= f;
            }
        }
    }

    fn etiqueta(&self, nonce: &[u8; 12], extra: &[u8], cifrado: &[u8]) -> [u8; 16] {
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;
        let mascara = u128::from_be_bytes(self.cifrar_bloco(&j0));
        (self.ghash(extra, cifrado) ^ mascara).to_be_bytes()
    }

    pub fn gcm_cifrar(&self, nonce: &[u8; 12], extra: &[u8], dados: &[u8]) -> Vec<u8> {
        let mut saida = dados.to_vec();
        self.contador(nonce, &mut saida);
        let etiqueta = self.etiqueta(nonce, extra, &saida);
        saida.extend_from_slice(&etiqueta);
        saida
    }

    pub fn gcm_decifrar(&self, nonce: &[u8; 12], extra: &[u8], cifrado: &[u8]) -> Option<Vec<u8>> {
        let n = cifrado.len().checked_sub(16)?;
        if !iguais(&self.etiqueta(nonce, extra, &cifrado[..n]), &cifrado[n..]) {
            return None;
        }
        let mut saida = cifrado[..n].to_vec();
        self.contador(nonce, &mut saida);
        Some(saida)
    }
}
