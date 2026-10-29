//! Alocador de quadros físicos: entrega páginas de 4 KiB da RAM.
//!
//! Um bitmap guarda 1 bit por quadro (1 = ocupado). Começa tudo ocupado e
//! liberamos só o que o mapa de memória do firmware diz ser RAM livre, menos
//! o próprio kernel e o que o bootloader deixou (initrd, estruturas).
//!
//! Só gerenciamos o primeiro 1 GiB, que está mapeado em identidade: assim o
//! kernel lê e escreve qualquer quadro usando o próprio endereço físico.
//!
//! Cada quadro também tem um contador de referências: depois de um `fork`,
//! pai e filho apontam para os mesmos quadros (copy-on-write), e um quadro
//! só volta a ficar livre quando o último dono o soltar.

use crate::interrupcoes::sem_interrupcoes;
use crate::multiboot::Info;
use crate::spin::Mutex;

pub const TAMANHO_PAGINA: u64 = 4096;
const LIMITE: u64 = 1 << 30;
const QUADROS: usize = (LIMITE / TAMANHO_PAGINA) as usize;

struct Bitmap {
    bits: [u64; QUADROS / 64],
    livres: usize,
    total: usize,
    dica: usize, // onde a última busca parou: evita varrer do começo toda vez
}

static BITMAP: Mutex<Bitmap> =
    Mutex::new(Bitmap { bits: [u64::MAX; QUADROS / 64], livres: 0, total: 0, dica: 0 });

/// Quantos espaços de endereçamento (ou tabelas) usam cada quadro.
/// Só mexemos nisto com o lock do BITMAP.
static mut REFERENCIAS: [u16; QUADROS] = [0; QUADROS];

fn referencias() -> &'static mut [u16; QUADROS] {
    unsafe { &mut *(&raw mut REFERENCIAS) }
}

unsafe extern "C" {
    /// Fim do kernel na memória (definido no linker.ld).
    static __fim_kernel: u8;
}

impl Bitmap {
    fn marcar(&mut self, quadro: usize, ocupado: bool) {
        let (palavra, bit) = (quadro / 64, quadro % 64);
        let estava = self.bits[palavra] & (1 << bit) != 0;
        if ocupado && !estava {
            self.bits[palavra] |= 1 << bit;
            self.livres -= 1;
        } else if !ocupado && estava {
            self.bits[palavra] &= !(1 << bit);
            self.livres += 1;
        }
    }

    fn reservar(&mut self, inicio: u64, fim: u64) {
        let mut q = inicio & !(TAMANHO_PAGINA - 1);
        while q < fim.min(LIMITE) {
            self.marcar((q / TAMANHO_PAGINA) as usize, true);
            q += TAMANHO_PAGINA;
        }
    }

    fn alocar(&mut self) -> Option<u64> {
        let n = self.bits.len();
        for k in 0..n {
            let i = (self.dica + k) % n;
            if self.bits[i] != u64::MAX {
                let bit = (!self.bits[i]).trailing_zeros() as usize;
                self.bits[i] |= 1 << bit;
                self.livres -= 1;
                self.dica = i;
                return Some((i * 64 + bit) as u64 * TAMANHO_PAGINA);
            }
        }
        None
    }
}

/// Monta o bitmap. Devolve quantos quadros ficaram livres.
pub fn init(info: &Info) -> usize {
    let mapa = info.mapa_memoria().expect("o bootloader não informou o mapa de memória");
    sem_interrupcoes(|| {
        let mut b = BITMAP.lock();
        for regiao in mapa.filter(|r| r.tipo == 1) {
            // Abaixo de 1 MiB mora BIOS, VGA e companhia: nem tentamos.
            let inicio = (regiao.inicio.max(0x10_0000) + TAMANHO_PAGINA - 1) & !(TAMANHO_PAGINA - 1);
            let fim = (regiao.inicio + regiao.tamanho).min(LIMITE) & !(TAMANHO_PAGINA - 1);
            let mut q = inicio;
            while q < fim {
                b.marcar((q / TAMANHO_PAGINA) as usize, false);
                b.total += 1;
                q += TAMANHO_PAGINA;
            }
        }
        b.reservar(0x10_0000, &raw const __fim_kernel as u64);
        info.reservas(|inicio, fim| b.reservar(inicio, fim));
        b.livres
    })
}

pub fn alocar() -> Option<u64> {
    sem_interrupcoes(|| {
        let quadro = BITMAP.lock().alocar()?;
        referencias()[(quadro / TAMANHO_PAGINA) as usize] = 1;
        Some(quadro)
    })
}

/// Um quadro novo, com os 4 KiB zerados.
pub fn alocar_zerado() -> Option<u64> {
    let quadro = alocar()?;
    unsafe { core::ptr::write_bytes(quadro as *mut u8, 0, TAMANHO_PAGINA as usize) };
    Some(quadro)
}

/// Solta uma referência; o quadro só fica livre quando era a última.
pub fn liberar(quadro: u64) {
    sem_interrupcoes(|| {
        let mut b = BITMAP.lock();
        let i = (quadro / TAMANHO_PAGINA) as usize;
        let r = &mut referencias()[i];
        if *r > 1 {
            *r -= 1;
            return;
        }
        *r = 0;
        b.marcar(i, false);
    });
}

/// Mais um dono para o quadro.
pub fn compartilhar(quadro: u64) {
    sem_interrupcoes(|| {
        let _b = BITMAP.lock();
        referencias()[(quadro / TAMANHO_PAGINA) as usize] += 1;
    });
}

pub fn donos(quadro: u64) -> u16 {
    sem_interrupcoes(|| {
        let _b = BITMAP.lock();
        referencias()[(quadro / TAMANHO_PAGINA) as usize]
    })
}

/// (total, livres), em quadros.
pub fn estatisticas() -> (usize, usize) {
    sem_interrupcoes(|| {
        let b = BITMAP.lock();
        (b.total, b.livres)
    })
}
