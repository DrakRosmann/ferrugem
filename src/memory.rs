//! Alocador de quadros físicos (páginas de 4 KiB) baseado em bitmap.
//!
//! O mapa direto do boot cobre os primeiros 4 GiB de RAM, então só
//! gerenciamos quadros abaixo desse limite.

use alloc::collections::BTreeMap;

use crate::multiboot::BootInfo;
use crate::paging::{self, KERNEL_VMA};
use crate::sync::SpinLock;

pub const FRAME_SIZE: u64 = 4096;
const MAX_FRAMES: usize = (4u64 << 30) as usize / FRAME_SIZE as usize;

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

struct FrameAllocator {
    /// Bit ligado = quadro livre. Começa zerado (tudo ocupado).
    bitmap: [u64; MAX_FRAMES / 64],
    total: usize,
    free: usize,
    /// Nenhuma palavra do bitmap antes desta tem quadro livre.
    hint: usize,
}

/// Referências extras a quadros compartilhados entre processos (cópia sob
/// escrita após um `fork`). Um quadro fora deste mapa tem um só dono.
static SHARED: SpinLock<BTreeMap<u64, u32>> = SpinLock::new(BTreeMap::new());

static FRAMES: SpinLock<FrameAllocator> =
    SpinLock::new(FrameAllocator { bitmap: [0; MAX_FRAMES / 64], total: 0, free: 0, hint: 0 });

impl FrameAllocator {
    fn is_free(&self, frame: usize) -> bool {
        self.bitmap[frame / 64] & (1 << (frame % 64)) != 0
    }

    fn set(&mut self, frame: usize, free: bool) {
        if frame >= MAX_FRAMES || self.is_free(frame) == free {
            return;
        }
        if free {
            self.bitmap[frame / 64] |= 1 << (frame % 64);
            self.free += 1;
            self.hint = self.hint.min(frame / 64);
        } else {
            self.bitmap[frame / 64] &= !(1 << (frame % 64));
            self.free -= 1;
        }
    }

    fn set_range(&mut self, start: u64, end: u64, free: bool) {
        for frame in (start / FRAME_SIZE) as usize..end.div_ceil(FRAME_SIZE).min(MAX_FRAMES as u64) as usize {
            self.set(frame, free);
        }
    }

    fn alloc_one(&mut self) -> Option<usize> {
        let word = (self.hint..self.bitmap.len()).find(|&i| self.bitmap[i] != 0)?;
        self.hint = word;
        let frame = word * 64 + self.bitmap[word].trailing_zeros() as usize;
        self.set(frame, false);
        Some(frame)
    }

    fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        let mut run = 0;
        for frame in 0..MAX_FRAMES {
            if self.is_free(frame) {
                run += 1;
                if run == count {
                    let first = frame + 1 - count;
                    for f in first..=frame {
                        self.set(f, false);
                    }
                    return Some(first);
                }
            } else {
                run = 0;
            }
        }
        None
    }
}

pub fn init(info: &BootInfo) {
    let mut frames = FRAMES.lock();
    for region in info.regions().iter().filter(|r| r.kind == 1) {
        // Só quadros inteiros dentro da região.
        let start = region.base.next_multiple_of(FRAME_SIZE);
        let end = (region.base + region.length) / FRAME_SIZE * FRAME_SIZE;
        if end > start {
            frames.set_range(start, end, true);
        }
    }
    frames.total = frames.free;

    // Reserva o primeiro MiB (BIOS, VGA...) e a imagem do kernel.
    let (kernel_start, kernel_end) = kernel_range();
    frames.set_range(0, 0x100000, false);
    frames.set_range(kernel_start, kernel_end, false);
}

/// Endereços físicos de início e fim da imagem do kernel.
pub fn kernel_range() -> (u64, u64) {
    (&raw const __kernel_start as u64 - KERNEL_VMA, &raw const __kernel_end as u64 - KERNEL_VMA)
}

/// Reserva `count` quadros contíguos e devolve o endereço do primeiro.
pub fn alloc_frames(count: usize) -> Option<u64> {
    FRAMES.lock().alloc_contiguous(count).map(|frame| frame as u64 * FRAME_SIZE)
}

/// Reserva um quadro e o preenche com zeros.
pub fn alloc_zeroed_frame() -> Option<u64> {
    let phys = FRAMES.lock().alloc_one()? as u64 * FRAME_SIZE;
    unsafe { core::ptr::write_bytes(paging::phys_to_virt::<u8>(phys), 0, FRAME_SIZE as usize) };
    Some(phys)
}

/// Solta uma referência ao quadro; ele só volta a ficar livre quando a
/// última referência for solta.
pub fn free_frame(phys: u64) {
    {
        let mut shared = SHARED.lock();
        if let Some(extra) = shared.get_mut(&phys) {
            *extra -= 1;
            if *extra == 0 {
                shared.remove(&phys);
            }
            return;
        }
    }
    FRAMES.lock().set((phys / FRAME_SIZE) as usize, true);
}

/// Acrescenta uma referência a um quadro já alocado.
pub fn share_frame(phys: u64) {
    *SHARED.lock().entry(phys).or_insert(0) += 1;
}

/// Quantos espaços de endereçamento usam o quadro.
pub fn frame_refs(phys: u64) -> u32 {
    1 + SHARED.lock().get(&phys).copied().unwrap_or(0)
}

/// (total, livres) em bytes.
pub fn stats() -> (u64, u64) {
    let frames = FRAMES.lock();
    (frames.total as u64 * FRAME_SIZE, frames.free as u64 * FRAME_SIZE)
}
