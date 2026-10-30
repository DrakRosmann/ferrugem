//! Conferir assinaturas digitais: RSA (PKCS#1 v1.5 e PSS, RFC 8017) e
//! ECDSA nas curvas P-256 e P-384 (FIPS 186-4).
//!
//! Só a parte pública: conferir, nunca assinar. É o que um cliente TLS
//! precisa para saber que o certificado foi emitido por quem diz e que o
//! servidor tem a chave privada do certificado.

use crate::numero::{self, Modulo, Num};
use crate::sha256::sha256;
use crate::sha512::{sha384, sha512};
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Resumo {
    Sha256,
    Sha384,
    Sha512,
}

impl Resumo {
    pub fn calcular(self, dados: &[u8]) -> Vec<u8> {
        match self {
            Resumo::Sha256 => sha256(dados).to_vec(),
            Resumo::Sha384 => sha384(dados),
            Resumo::Sha512 => sha512(dados),
        }
    }

    pub fn tamanho(self) -> usize {
        match self {
            Resumo::Sha256 => 32,
            Resumo::Sha384 => 48,
            Resumo::Sha512 => 64,
        }
    }
}

// -----------------------------------------------------------------------------
// RSA
// -----------------------------------------------------------------------------

/// assinatura^e mod n, em `k` bytes (k = tamanho de n). None se a
/// assinatura não tiver o tamanho certo ou não for menor que n.
fn rsa_abrir(n: &[u8], e: &[u8], assinatura: &[u8]) -> Option<Vec<u8>> {
    let n = &n[n.iter().position(|&b| b != 0)?..];
    if n.len() < 128 || n.len() > 1024 || n[n.len() - 1] & 1 == 0 || assinatura.len() != n.len() {
        return None; // menos de 1024 bits não é seguro; mais de 8192 ninguém usa
    }
    let m = Modulo::novo(n);
    let s = m.ler(assinatura)?;
    Some(numero::para_bytes(&m.normal(&m.potencia(&m.mont(&s), e)), n.len()))
}

/// PKCS#1 v1.5: 00 01 FF…FF 00 + "DigestInfo" (qual hash) + o hash.
pub fn rsa_pkcs1(n: &[u8], e: &[u8], resumo: Resumo, hash: &[u8], assinatura: &[u8]) -> bool {
    let Some(em) = rsa_abrir(n, e, assinatura) else { return false };
    let prefixo: &[u8] = match resumo {
        Resumo::Sha256 => &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20],
        Resumo::Sha384 => &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30],
        Resumo::Sha512 => &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40],
    };
    let t = prefixo.len() + hash.len();
    if em.len() < t + 11 {
        return false;
    }
    let mut esperado = alloc::vec![0xFFu8; em.len()];
    esperado[0] = 0;
    esperado[1] = 1;
    esperado[em.len() - t - 1] = 0;
    esperado[em.len() - t..em.len() - hash.len()].copy_from_slice(prefixo);
    esperado[em.len() - hash.len()..].copy_from_slice(hash);
    em == esperado
}

/// MGF1: estica uma semente em `tamanho` bytes de máscara.
fn mgf1(resumo: Resumo, semente: &[u8], tamanho: usize) -> Vec<u8> {
    let mut saida = Vec::new();
    let mut contador = 0u32;
    while saida.len() < tamanho {
        let mut entrada = semente.to_vec();
        entrada.extend_from_slice(&contador.to_be_bytes());
        saida.extend(resumo.calcular(&entrada));
        contador += 1;
    }
    saida.truncate(tamanho);
    saida
}

/// PSS (o que o TLS 1.3 usa com chaves RSA), com sal do tamanho do hash.
pub fn rsa_pss(n: &[u8], e: &[u8], resumo: Resumo, hash: &[u8], assinatura: &[u8]) -> bool {
    let Some(em) = rsa_abrir(n, e, assinatura) else { return false };
    let n = &n[n.iter().position(|&b| b != 0).unwrap_or(0)..];
    let bits = (n.len() - 1) * 8 + (8 - n[0].leading_zeros() as usize) - 1; // emBits
    let tamanho = bits.div_ceil(8);
    let (sobra, em) = em.split_at(em.len() - tamanho);
    let h = resumo.tamanho();
    let sal = h;
    if sobra.iter().any(|&b| b != 0) || tamanho < h + sal + 2 || em[tamanho - 1] != 0xbc {
        return false;
    }
    let (mascarado, resto) = em.split_at(tamanho - h - 1);
    let hh = &resto[..h];
    let zeros_em_cima = 8 * tamanho - bits;
    if zeros_em_cima > 0 && mascarado[0] >> (8 - zeros_em_cima) != 0 {
        return false;
    }
    let mut db: Vec<u8> = mascarado.iter().zip(mgf1(resumo, hh, mascarado.len())).map(|(a, b)| a ^ b).collect();
    db[0] &= 0xFF >> zeros_em_cima;
    let separador = db.len() - sal - 1;
    if db[..separador].iter().any(|&b| b != 0) || db[separador] != 1 {
        return false;
    }
    let mut m = alloc::vec![0u8; 8];
    m.extend_from_slice(hash);
    m.extend_from_slice(&db[separador + 1..]);
    resumo.calcular(&m) == hh
}

// -----------------------------------------------------------------------------
// ECDSA: curvas y² = x³ - 3x + b sobre os inteiros módulo um primo p
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NomeCurva {
    P256,
    P384,
}

struct Curva {
    p: Modulo,
    /// A ordem do ponto gerador G (quantos múltiplos de G existem).
    n: Modulo,
    b: Num,
    g: Ponto,
    bytes: usize,
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

impl Curva {
    fn nova(nome: NomeCurva) -> Curva {
        let [p, b, n, gx, gy] = match nome {
            NomeCurva::P256 => [
                "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
                "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
                "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
                "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
                "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
            ],
            NomeCurva::P384 => [
                "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
                "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
                "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
                "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
                "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
            ],
        };
        let p = Modulo::novo(&hex(p));
        let n = Modulo::novo(&hex(n));
        let k = p.palavras();
        let b = p.mont(&numero::de_bytes(&hex(b), k));
        let g = Ponto { x: p.mont(&numero::de_bytes(&hex(gx), k)), y: p.mont(&numero::de_bytes(&hex(gy), k)), z: p.um.clone() };
        Curva { bytes: k * 8, p, n, b, g }
    }

    /// (x, y) (forma de Montgomery) está na curva?
    fn contem(&self, x: &Num, y: &Num) -> bool {
        let p = &self.p;
        let x3 = p.mul(&p.mul(x, x), x);
        let tres_x = p.somar(&p.somar(x, x), x);
        p.mul(y, y) == p.somar(&p.subtrair(&x3, &tres_x), &self.b)
    }

    /// 2P, em coordenadas jacobianas (x = X/Z², y = Y/Z³: sem divisões no meio).
    fn dobrar(&self, a: &Ponto) -> Ponto {
        let p = &self.p;
        if numero::zero(&a.z) || numero::zero(&a.y) {
            return Ponto::infinito(p.palavras());
        }
        let delta = p.mul(&a.z, &a.z);
        let gama = p.mul(&a.y, &a.y);
        let beta = p.mul(&a.x, &gama);
        let t = p.mul(&p.subtrair(&a.x, &delta), &p.somar(&a.x, &delta));
        let alfa = p.somar(&p.somar(&t, &t), &t); // 3(x - z²)(x + z²): é aqui que entra o "-3"
        let beta4 = p.somar(&p.somar(&beta, &beta), &p.somar(&beta, &beta));
        let beta8 = p.somar(&beta4, &beta4);
        let x = p.subtrair(&p.mul(&alfa, &alfa), &beta8);
        let yz = p.somar(&a.y, &a.z);
        let z = p.subtrair(&p.subtrair(&p.mul(&yz, &yz), &gama), &delta);
        let gama2 = p.mul(&gama, &gama);
        let gama8 = {
            let g2 = p.somar(&gama2, &gama2);
            let g4 = p.somar(&g2, &g2);
            p.somar(&g4, &g4)
        };
        let y = p.subtrair(&p.mul(&alfa, &p.subtrair(&beta4, &x)), &gama8);
        Ponto { x, y, z }
    }

    /// P + Q.
    fn somar(&self, a: &Ponto, b: &Ponto) -> Ponto {
        let p = &self.p;
        if numero::zero(&a.z) {
            return b.clone();
        }
        if numero::zero(&b.z) {
            return a.clone();
        }
        let z1z1 = p.mul(&a.z, &a.z);
        let z2z2 = p.mul(&b.z, &b.z);
        let u1 = p.mul(&a.x, &z2z2);
        let u2 = p.mul(&b.x, &z1z1);
        let s1 = p.mul(&p.mul(&a.y, &b.z), &z2z2);
        let s2 = p.mul(&p.mul(&b.y, &a.z), &z1z1);
        let h = p.subtrair(&u2, &u1);
        let r = p.subtrair(&s2, &s1);
        if numero::zero(&h) {
            return if numero::zero(&r) { self.dobrar(a) } else { Ponto::infinito(p.palavras()) };
        }
        let h2 = p.mul(&h, &h);
        let h3 = p.mul(&h, &h2);
        let u1h2 = p.mul(&u1, &h2);
        let x = p.subtrair(&p.subtrair(&p.mul(&r, &r), &h3), &p.somar(&u1h2, &u1h2));
        let y = p.subtrair(&p.mul(&r, &p.subtrair(&u1h2, &x)), &p.mul(&s1, &h3));
        let z = p.mul(&p.mul(&a.z, &b.z), &h);
        Ponto { x, y, z }
    }
}

#[derive(Clone)]
struct Ponto {
    x: Num,
    y: Num,
    z: Num,
}

impl Ponto {
    fn infinito(palavras: usize) -> Ponto {
        Ponto { x: alloc::vec![0; palavras], y: alloc::vec![0; palavras], z: alloc::vec![0; palavras] }
    }
}

/// Os dois inteiros (r, s) de uma assinatura ECDSA em DER: SEQUENCE { INTEGER, INTEGER }.
fn ler_r_s(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut externo = crate::x509::Der::novo(der);
    let mut d = crate::x509::Der::novo(externo.esperar(0x30)?);
    let r = d.esperar(0x02)?;
    let s = d.esperar(0x02)?;
    (externo.vazio() && d.vazio()).then_some((r, s))
}

/// Confere uma assinatura ECDSA: `publica` é o ponto sem compressão
/// (04 ‖ x ‖ y), `hash` o resumo da mensagem, `assinatura` o (r, s) em DER.
pub fn ecdsa(curva: NomeCurva, publica: &[u8], hash: &[u8], assinatura: &[u8]) -> bool {
    let c = Curva::nova(curva);
    let (p, n, k) = (&c.p, &c.n, c.p.palavras());
    // A chave pública: um ponto da curva.
    if publica.len() != 1 + 2 * c.bytes || publica[0] != 4 {
        return false;
    }
    let (Some(qx), Some(qy)) = (p.ler(&publica[1..1 + c.bytes]), p.ler(&publica[1 + c.bytes..])) else { return false };
    let (qx, qy) = (p.mont(&qx), p.mont(&qy));
    if !c.contem(&qx, &qy) {
        return false;
    }
    let q = Ponto { x: qx, y: qy, z: p.um.clone() };
    // r e s entre 1 e n-1.
    let Some((r, s)) = ler_r_s(assinatura) else { return false };
    let (Some(r), Some(s)) = (n.ler(r), n.ler(s)) else { return false };
    if numero::zero(&r) || numero::zero(&s) {
        return false;
    }
    // e = os bits mais altos do hash (tantos quantos os de n).
    let mut e = numero::de_bytes(&hash[..hash.len().min(c.bytes)], k);
    if !numero::menor(&e, &n.m) {
        numero::subtrair(&mut e, &n.m);
    }
    // u1 = e/s, u2 = r/s (mod n); o ponto u1·G + u2·Q tem que ter x ≡ r.
    let w = n.inverso(&n.mont(&s));
    let u1 = n.normal(&n.mul(&n.mont(&e), &w));
    let u2 = n.normal(&n.mul(&n.mont(&r), &w));
    // Os dois produtos de uma vez ("truque de Shamir"): dobra e soma G, Q ou G+Q.
    let gq = c.somar(&c.g, &q);
    let mut x = Ponto::infinito(k);
    for i in (0..64 * k).rev() {
        x = c.dobrar(&x);
        match (numero::bit(&u1, i), numero::bit(&u2, i)) {
            (true, true) => x = c.somar(&x, &gq),
            (true, false) => x = c.somar(&x, &c.g),
            (false, true) => x = c.somar(&x, &q),
            _ => {}
        }
    }
    if numero::zero(&x.z) {
        return false;
    }
    let zi = p.inverso(&x.z);
    let mut afim = p.normal(&p.mul(&x.x, &p.mul(&zi, &zi)));
    if !numero::menor(&afim, &n.m) {
        numero::subtrair(&mut afim, &n.m);
    }
    afim == r
}
