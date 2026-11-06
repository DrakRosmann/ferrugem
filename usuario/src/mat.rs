//! Matemática de ponto flutuante sem a `std` (o `core` não tem `sqrt`, `sin`
//! nem `floor`). Tudo em Rust puro, sobre os bits do IEEE 754: os programas em
//! Rust são compilados em soft-float (o alvo x86_64-unknown-none não usa SSE).

const SINAL: u64 = 1 << 63;
pub const PI: f64 = 3.141592653589793;
pub const E: f64 = 2.718281828459045;
const LN2: f64 = 0.6931471805599453;

pub fn trunc(x: f64) -> f64 {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i32 - 1023;
    if exp >= 52 {
        return x;
    }
    if exp < 0 {
        return f64::from_bits(bits & SINAL);
    }
    f64::from_bits(bits & !((1u64 << (52 - exp)) - 1))
}

pub fn floor(x: f64) -> f64 {
    let t = trunc(x);
    if t > x { t - 1.0 } else { t }
}

pub fn ceil(x: f64) -> f64 {
    let t = trunc(x);
    if t < x {
        let r = t + 1.0;
        // -0.5 arredonda para -0
        if r == 0.0 && x < 0.0 { -0.0 } else { r }
    } else {
        t
    }
}

/// `Math.round` do JavaScript: .5 vai para cima.
pub fn round(x: f64) -> f64 {
    if !x.is_finite() || abs(x) >= 4503599627370496.0 {
        return x;
    }
    let r = floor(x);
    let v = if x - r >= 0.5 { r + 1.0 } else { r };
    if v == 0.0 && x < 0.0 { -0.0 } else { v }
}

pub fn abs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !SINAL)
}

/// O resto com o sinal do dividendo (o `%` do JavaScript).
pub fn fmod(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() || x.is_infinite() || y == 0.0 {
        return f64::NAN;
    }
    if y.is_infinite() || x == 0.0 {
        return x;
    }
    let sinal = x.to_bits() & SINAL;
    let (mut ux, mut uy) = (x.to_bits() & !SINAL, y.to_bits() & !SINAL);
    if ux < uy {
        return x;
    }
    let mut ex = (ux >> 52) as i32;
    let mut ey = (uy >> 52) as i32;
    if ex == 0 {
        let z = ux.leading_zeros() as i32 - 11;
        ux <<= z;
        ex = 1 - z;
    } else {
        ux = (ux & ((1 << 52) - 1)) | (1 << 52);
    }
    if ey == 0 {
        let z = uy.leading_zeros() as i32 - 11;
        uy <<= z;
        ey = 1 - z;
    } else {
        uy = (uy & ((1 << 52) - 1)) | (1 << 52);
    }
    while ex > ey {
        if ux >= uy {
            ux -= uy;
        }
        if ux == 0 {
            return f64::from_bits(sinal);
        }
        ux <<= 1;
        ex -= 1;
    }
    if ux >= uy {
        ux -= uy;
    }
    if ux == 0 {
        return f64::from_bits(sinal);
    }
    let z = ux.leading_zeros() as i32 - 11;
    ux <<= z;
    ex -= z;
    let r = if ex > 0 { (ux & ((1 << 52) - 1)) | ((ex as u64) << 52) } else { ux >> (1 - ex) };
    f64::from_bits(r | sinal)
}

pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    // Raiz inteira, bit a bit, de um número de ~110 bits: dá 55 bits exatos,
    // e o último guarda se sobrou resto (o arredondamento fica correto).
    let (m, e) = frexp(x); // x = m * 2^e, m em [0.5, 1)
    let mut inteiro = (m.to_bits() & ((1 << 52) - 1)) | (1 << 52); // m * 2^53
    let mut exp = e - 53;
    if exp & 1 != 0 {
        inteiro <<= 1;
        exp -= 1;
    }
    let mut n = (inteiro as u128) << 56;
    exp -= 56;
    let mut r: u128 = 0;
    let mut bit: u128 = 1 << 112;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if n >= r + bit {
            n -= r + bit;
            r = (r >> 1) + bit;
        } else {
            r >>= 1;
        }
        bit >>= 2;
    }
    let com_resto = (r as u64) | (n != 0) as u64;
    ldexp(com_resto as f64, exp / 2)
}

/// x = m * 2^e, com m em [0.5, 1).
fn frexp(x: f64) -> (f64, i32) {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i32;
    if exp == 0 {
        let (m, e) = frexp(x * 18014398509481984.0); // 2^54
        return (m, e - 54);
    }
    (f64::from_bits((bits & !(0x7FF << 52)) | (1022 << 52)), exp - 1022)
}

pub fn ldexp(mut x: f64, mut e: i32) -> f64 {
    while e > 1000 {
        x *= f64::from_bits(((1000 + 1023) as u64) << 52);
        e -= 1000;
        if x.is_infinite() {
            return x;
        }
    }
    while e < -1000 {
        x *= f64::from_bits(((-1000 + 1023) as u64) << 52);
        e += 1000;
        if x == 0.0 {
            return x;
        }
    }
    x * f64::from_bits(((e + 1023) as u64) << 52)
}

pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.8 {
        return f64::INFINITY;
    }
    if x < -745.2 {
        return 0.0;
    }
    let n = floor(x / LN2 + 0.5);
    let r = x - n * LN2;
    // Taylor em |r| <= ln2/2
    let mut termo = 1.0;
    let mut soma = 1.0;
    for i in 1..16 {
        termo *= r / i as f64;
        soma += termo;
    }
    ldexp(soma, n as i32)
}

pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return x;
    }
    let (mut m, mut e) = frexp(x);
    if m < 0.70710678118654752 {
        m *= 2.0;
        e -= 1;
    }
    // ln(m) = 2 atanh((m-1)/(m+1))
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut termo = z;
    let mut soma = 0.0;
    let mut i = 1;
    while i < 40 {
        soma += termo / i as f64;
        termo *= z2;
        i += 2;
    }
    2.0 * soma + e as f64 * LN2
}

pub fn log10(x: f64) -> f64 {
    let r = ln(x) * 0.4342944819032518;
    let a = round(r);
    if abs(r - a) < 1e-13 && pow(10.0, a) == x { a } else { r }
}

pub fn log2(x: f64) -> f64 {
    if x > 0.0 && x.is_finite() {
        let (m, e) = frexp(x);
        if m == 0.5 {
            return (e - 1) as f64;
        }
    }
    ln(x) * 1.4426950408889634
}

pub fn pow(b: f64, e: f64) -> f64 {
    if e == 0.0 {
        return 1.0;
    }
    if b.is_nan() || e.is_nan() {
        return f64::NAN;
    }
    if e.is_infinite() {
        let a = abs(b);
        return if a == 1.0 {
            f64::NAN
        } else if (a > 1.0) == (e > 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
    }
    let e_inteiro = trunc(e) == e;
    if e_inteiro && abs(e) <= 1024.0 {
        let mut n = abs(e) as u32;
        let (mut r, mut base) = (1.0f64, b);
        while n > 0 {
            if n & 1 != 0 {
                r *= base;
            }
            base *= base;
            n >>= 1;
        }
        return if e < 0.0 { 1.0 / r } else { r };
    }
    if b == 0.0 {
        return if e < 0.0 { f64::INFINITY } else { 0.0 };
    }
    if b.is_infinite() {
        let impar = e_inteiro && fmod(e, 2.0) != 0.0;
        let r = if e > 0.0 { f64::INFINITY } else { 0.0 };
        return if b < 0.0 && impar { -r } else { r };
    }
    if b < 0.0 {
        if !e_inteiro {
            return f64::NAN;
        }
        let r = exp(e * ln(-b));
        return if fmod(e, 2.0) != 0.0 { -r } else { r };
    }
    exp(e * ln(b))
}

/// Reduz o ângulo: x = k*(pi/2) + r, com |r| <= pi/4.
fn reduzir(x: f64) -> (i64, f64) {
    const PI2_A: f64 = 1.5707963267341256;
    const PI2_B: f64 = 6.077100506506192e-11;
    const PI2_C: f64 = 2.0222662487959506e-21;
    let k = floor(x / 1.5707963267948966 + 0.5);
    (k as i64, ((x - k * PI2_A) - k * PI2_B) - k * PI2_C)
}

fn seno_k(r: f64) -> f64 {
    let z = r * r;
    r + r * z * (-1.6666666666666632e-01 + z * (8.3333333333224894e-03 + z * (-1.9841269829857949e-04 + z * (2.7557316103728e-06 + z * (-2.5051132049748e-08 + z * 1.58969e-10)))))
}

fn cosseno_k(r: f64) -> f64 {
    let z = r * r;
    1.0 - 0.5 * z + z * z * (4.1666666666666019e-02 + z * (-1.3888888888874e-03 + z * (2.4801587288e-05 + z * (-2.75573e-07 + z * (2.08757e-09 - z * 1.1359e-11)))))
}

pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x == 0.0 {
        return x;
    }
    let (k, r) = reduzir(x);
    match k.rem_euclid(4) {
        0 => seno_k(r),
        1 => cosseno_k(r),
        2 => -seno_k(r),
        _ => -cosseno_k(r),
    }
}

pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (k, r) = reduzir(x);
    match k.rem_euclid(4) {
        0 => cosseno_k(r),
        1 => -seno_k(r),
        2 => -cosseno_k(r),
        _ => seno_k(r),
    }
}

pub fn tan(x: f64) -> f64 {
    sin(x) / cos(x)
}

pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x.is_infinite() {
        return if x > 0.0 { PI / 2.0 } else { -PI / 2.0 };
    }
    if x == 0.0 {
        return x;
    }
    let (negativo, a) = (x < 0.0, abs(x));
    let (inverte, a) = if a > 1.0 { (true, 1.0 / a) } else { (false, a) };
    // Duas reduções: atan(a) = 2 atan(a / (1 + sqrt(1 + a²))).
    let mut t = a;
    let mut fator = 1.0;
    for _ in 0..3 {
        t = t / (1.0 + sqrt(1.0 + t * t));
        fator *= 2.0;
    }
    let z = t * t;
    let mut r = t * (1.0 - z * (1.0 / 3.0 - z * (1.0 / 5.0 - z * (1.0 / 7.0 - z * (1.0 / 9.0 - z * (1.0 / 11.0 - z * (1.0 / 13.0)))))));
    r *= fator;
    if inverte {
        r = PI / 2.0 - r;
    }
    if negativo { -r } else { r }
}

pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let negativo_y = y.is_sign_negative();
    if y == 0.0 {
        let r = if x.is_sign_negative() { PI } else { 0.0 };
        return if negativo_y { -r } else { r };
    }
    if x == 0.0 {
        return if negativo_y { -PI / 2.0 } else { PI / 2.0 };
    }
    if x.is_infinite() && y.is_infinite() {
        let r = if x > 0.0 { PI / 4.0 } else { 3.0 * PI / 4.0 };
        return if negativo_y { -r } else { r };
    }
    let r = atan(y / x);
    if x > 0.0 {
        r
    } else if negativo_y {
        r - PI
    } else {
        r + PI
    }
}

pub fn asin(x: f64) -> f64 {
    if x.is_nan() || abs(x) > 1.0 {
        return f64::NAN;
    }
    if abs(x) == 1.0 {
        return x * PI / 2.0;
    }
    atan(x / sqrt(1.0 - x * x))
}

pub fn acos(x: f64) -> f64 {
    if x.is_nan() || abs(x) > 1.0 {
        return f64::NAN;
    }
    PI / 2.0 - asin(x)
}

pub fn sinh(x: f64) -> f64 {
    if abs(x) < 1e-5 {
        return x;
    }
    (exp(x) - exp(-x)) / 2.0
}

pub fn cosh(x: f64) -> f64 {
    (exp(x) + exp(-x)) / 2.0
}

pub fn tanh(x: f64) -> f64 {
    if x > 20.0 {
        return 1.0;
    }
    if x < -20.0 {
        return -1.0;
    }
    sinh(x) / cosh(x)
}

pub fn cbrt(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let a = abs(x);
    let mut r = exp(ln(a) / 3.0);
    r = r - (r * r * r - a) / (3.0 * r * r);
    r = r - (r * r * r - a) / (3.0 * r * r);
    if x < 0.0 { -r } else { r }
}
