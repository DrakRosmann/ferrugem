//! Números grandes (de 256 a 8192 bits) e contas "módulo m", para conferir
//! assinaturas RSA e ECDSA.
//!
//! Um número é um vetor de palavras de 64 bits, a menos significativa
//! primeiro. As multiplicações usam a forma de Montgomery: em vez de `a`,
//! guarda-se `a·R mod m` (R = 2^(64·palavras)); assim, multiplicar e reduzir
//! módulo m vira multiplicar e deslocar, sem nenhuma divisão.

use alloc::vec;
use alloc::vec::Vec;

pub type Num = Vec<u64>;

/// Bytes "big-endian" (como vêm nos certificados) → número com `palavras` palavras.
/// Bytes que não cabem são ignorados (quem chama confere o tamanho antes).
pub fn de_bytes(bytes: &[u8], palavras: usize) -> Num {
    let mut x = vec![0u64; palavras];
    for (i, &b) in bytes.iter().rev().enumerate() {
        if i / 8 < palavras {
            x[i / 8] |= (b as u64) << (8 * (i % 8));
        }
    }
    x
}

/// Número → `tamanho` bytes "big-endian".
pub fn para_bytes(x: &[u64], tamanho: usize) -> Vec<u8> {
    (0..tamanho).rev().map(|i| x.get(i / 8).map_or(0, |p| (p >> (8 * (i % 8))) as u8)).collect()
}

pub fn zero(x: &[u64]) -> bool {
    x.iter().all(|&p| p == 0)
}

pub fn bit(x: &[u64], i: usize) -> bool {
    x.get(i / 64).is_some_and(|p| p >> (i % 64) & 1 == 1)
}

/// a < b (mesmo número de palavras).
pub fn menor(a: &[u64], b: &[u64]) -> bool {
    for (x, y) in a.iter().rev().zip(b.iter().rev()) {
        if x != y {
            return x < y;
        }
    }
    false
}

/// a -= b; devolve o "empresta um" que sobrou.
pub fn subtrair(a: &mut [u64], b: &[u64]) -> bool {
    let mut emprestimo = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (d, e1) = x.overflowing_sub(y);
        let (d, e2) = d.overflowing_sub(emprestimo as u64);
        *x = d;
        emprestimo = e1 || e2;
    }
    emprestimo
}

fn somar(a: &mut [u64], b: &[u64]) -> bool {
    let mut vai_um = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (s, c1) = x.overflowing_add(y);
        let (s, c2) = s.overflowing_add(vai_um as u64);
        *x = s;
        vai_um = c1 || c2;
    }
    vai_um
}

/// Um módulo ímpar (o primo de uma curva, a ordem dela, o n do RSA) e as
/// constantes que a forma de Montgomery pede.
pub struct Modulo {
    pub m: Num,
    /// -m⁻¹ mod 2^64
    inverso: u64,
    /// R² mod m: multiplicar por ele leva um número para a forma de Montgomery.
    r2: Num,
    /// R mod m: o 1 na forma de Montgomery.
    pub um: Num,
}

impl Modulo {
    /// `bytes` é o módulo em "big-endian"; precisa ser ímpar.
    pub fn novo(bytes: &[u8]) -> Modulo {
        let palavras = bytes.len().div_ceil(8).max(1);
        let m = de_bytes(bytes, palavras);
        // Newton: cada passo dobra os bits certos (m·m ≡ 1 mod 8 já dá 3).
        let mut x = m[0];
        for _ in 0..5 {
            x = x.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(x)));
        }
        // R mod m e R² mod m, dobrando o 1 e reduzindo a cada passo.
        let mut r = vec![0u64; palavras];
        r[0] = 1;
        let mut um = Vec::new();
        for i in 0..2 * 64 * palavras {
            let mut vai_um = false;
            for p in r.iter_mut() {
                let novo_vai_um = *p >> 63 == 1;
                *p = *p << 1 | vai_um as u64;
                vai_um = novo_vai_um;
            }
            if vai_um || !menor(&r, &m) {
                subtrair(&mut r, &m);
            }
            if i + 1 == 64 * palavras {
                um = r.clone();
            }
        }
        Modulo { m, inverso: x.wrapping_neg(), r2: r, um }
    }

    pub fn palavras(&self) -> usize {
        self.m.len()
    }

    /// Lê um número em bytes; None se ele não for menor que o módulo.
    pub fn ler(&self, bytes: &[u8]) -> Option<Num> {
        let sem_zeros = &bytes[bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len())..];
        if sem_zeros.len() > self.palavras() * 8 {
            return None;
        }
        let x = de_bytes(sem_zeros, self.palavras());
        menor(&x, &self.m).then_some(x)
    }

    /// a·b·R⁻¹ mod m (o produto de Montgomery, "CIOS"): com os dois na forma
    /// de Montgomery, o resultado também está nela.
    pub fn mul(&self, a: &[u64], b: &[u64]) -> Num {
        let n = self.palavras();
        let m = &self.m;
        let mut t = vec![0u64; n + 2];
        for &ai in a.iter().take(n) {
            // t += ai·b
            let mut c = 0u128;
            for j in 0..n {
                let s = t[j] as u128 + ai as u128 * b[j] as u128 + c;
                t[j] = s as u64;
                c = s >> 64;
            }
            let s = t[n] as u128 + c;
            t[n] = s as u64;
            t[n + 1] = (s >> 64) as u64;
            // t += q·m, com q escolhido para zerar a palavra de baixo; desloca uma palavra.
            let q = t[0].wrapping_mul(self.inverso);
            let s = t[0] as u128 + q as u128 * m[0] as u128;
            let mut c = s >> 64;
            for j in 1..n {
                let s = t[j] as u128 + q as u128 * m[j] as u128 + c;
                t[j - 1] = s as u64;
                c = s >> 64;
            }
            let s = t[n] as u128 + c;
            t[n - 1] = s as u64;
            t[n] = t[n + 1] + (s >> 64) as u64;
        }
        let mut r = t[..n].to_vec();
        if t[n] != 0 || !menor(&r, m) {
            subtrair(&mut r, m);
        }
        r
    }

    /// Para a forma de Montgomery.
    pub fn mont(&self, a: &[u64]) -> Num {
        self.mul(a, &self.r2)
    }

    /// De volta da forma de Montgomery.
    pub fn normal(&self, a: &[u64]) -> Num {
        let mut um = vec![0u64; self.palavras()];
        um[0] = 1;
        self.mul(a, &um)
    }

    pub fn somar(&self, a: &[u64], b: &[u64]) -> Num {
        let mut r = a.to_vec();
        if somar(&mut r, b) || !menor(&r, &self.m) {
            subtrair(&mut r, &self.m);
        }
        r
    }

    pub fn subtrair(&self, a: &[u64], b: &[u64]) -> Num {
        let mut r = a.to_vec();
        if subtrair(&mut r, b) {
            somar(&mut r, &self.m);
        }
        r
    }

    /// base^expoente (base na forma de Montgomery; o expoente em bytes "big-endian").
    pub fn potencia(&self, base: &[u64], expoente: &[u8]) -> Num {
        let mut r = self.um.clone();
        for &byte in expoente {
            for i in (0..8).rev() {
                r = self.mul(&r, &r);
                if byte >> i & 1 == 1 {
                    r = self.mul(&r, base);
                }
            }
        }
        r
    }

    /// a⁻¹ (forma de Montgomery), pelo pequeno teorema de Fermat: a^(m-2).
    /// Só vale com m primo.
    pub fn inverso(&self, a: &[u64]) -> Num {
        let mut e = self.m.clone();
        let mut dois = vec![0u64; self.palavras()];
        dois[0] = 2;
        subtrair(&mut e, &dois);
        self.potencia(a, &para_bytes(&e, self.palavras() * 8))
    }
}
