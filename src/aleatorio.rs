//! Números aleatórios: `/dev/random`, `/dev/urandom` e a syscall `getrandom`.
//!
//! Se a CPU tiver a instrução RDRAND (um gerador de verdade, dentro do
//! processador), usamos ela. Senão, um xorshift alimentado pelo contador de
//! ciclos (TSC): imprevisível o bastante para sementes e testes, mas *não*
//! para criptografia.

use core::arch::asm;
use core::arch::x86_64::{__cpuid, _rdtsc};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static TEM_RDRAND: AtomicBool = AtomicBool::new(false);
static ESTADO: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

pub fn init() {
    #[allow(unused_unsafe)]
    let ecx = unsafe { __cpuid(1) }.ecx;
    TEM_RDRAND.store(ecx & (1 << 30) != 0, Ordering::Relaxed);
    ESTADO.store(unsafe { _rdtsc() } | 1, Ordering::Relaxed);
}

pub fn tem_hardware() -> bool {
    TEM_RDRAND.load(Ordering::Relaxed)
}

fn rdrand() -> Option<u64> {
    // A instrução pode falhar se o gerador estiver momentaneamente sem
    // entropia (o carry vem zerado): a Intel recomenda tentar umas 10 vezes.
    for _ in 0..10 {
        let (valor, ok): (u64, u8);
        unsafe { asm!("rdrand {}", "setc {}", out(reg) valor, out(reg_byte) ok, options(nomem, nostack)) };
        if ok == 1 {
            return Some(valor);
        }
    }
    None
}

pub fn proximo() -> u64 {
    if tem_hardware() {
        if let Some(v) = rdrand() {
            return v;
        }
    }
    // xorshift64*, misturando o TSC a cada chamada
    let mut x = ESTADO.load(Ordering::Relaxed) ^ unsafe { _rdtsc() }.rotate_left(17);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    ESTADO.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

pub fn preencher(buf: &mut [u8]) {
    for pedaco in buf.chunks_mut(8) {
        let bytes = proximo().to_le_bytes();
        pedaco.copy_from_slice(&bytes[..pedaco.len()]);
    }
}
