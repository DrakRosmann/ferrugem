//! Heap do kernel: alocador de lista livre (first-fit), ordenada por endereço,
//! que funde blocos vizinhos ao liberar. Habilita `Box`, `Vec`, `String`...

use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ptr::null_mut;

use crate::memory;
use crate::paging;
use crate::sync::SpinLock;

/// Toda alocação é múltipla de 16 bytes e alinhada a 16, e todo bloco livre
/// cabe um cabeçalho `FreeBlock`; assim as sobras são sempre reutilizáveis.
const GRANULE: usize = 16;

struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

struct Heap {
    /// Nó sentinela: `head.next` é o primeiro bloco livre.
    head: FreeBlock,
    size: usize,
    used: usize,
}

unsafe impl Send for Heap {}

struct KernelAllocator(SpinLock<Heap>);

#[global_allocator]
static ALLOCATOR: KernelAllocator =
    KernelAllocator(SpinLock::new(Heap { head: FreeBlock { size: 0, next: null_mut() }, size: 0, used: 0 }));

const _: () = assert!(size_of::<FreeBlock>() <= GRANULE);

fn round_size(layout: &Layout) -> usize {
    layout.size().max(1).next_multiple_of(GRANULE)
}

impl Heap {
    unsafe fn add_free(&mut self, addr: usize, size: usize) {
        // Encontra o vizinho anterior (por endereço) e insere depois dele.
        let mut prev: *mut FreeBlock = &mut self.head;
        unsafe {
            while !(*prev).next.is_null() && ((*prev).next as usize) < addr {
                prev = (*prev).next;
            }
            let block = addr as *mut FreeBlock;
            block.write(FreeBlock { size, next: (*prev).next });
            (*prev).next = block;

            // Funde com o próximo.
            let next = (*block).next;
            if !next.is_null() && addr + (*block).size == next as usize {
                (*block).size += (*next).size;
                (*block).next = (*next).next;
            }
            // Funde com o anterior (o sentinela nunca é fundido).
            if prev != &mut self.head as *mut _ && prev as usize + (*prev).size == addr {
                (*prev).size += (*block).size;
                (*prev).next = (*block).next;
            }
        }
    }

    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = round_size(&layout);
        let align = layout.align().max(GRANULE);
        let mut prev: *mut FreeBlock = &mut self.head;
        unsafe {
            while !(*prev).next.is_null() {
                let block = (*prev).next;
                let block_start = block as usize;
                let block_end = block_start + (*block).size;
                let start = block_start.next_multiple_of(align);
                let end = start + size;
                if end <= block_end {
                    // Retira o bloco e devolve as sobras da frente e de trás.
                    (*prev).next = (*block).next;
                    if start > block_start {
                        self.add_free(block_start, start - block_start);
                    }
                    if block_end > end {
                        self.add_free(end, block_end - end);
                    }
                    self.used += size;
                    return start as *mut u8;
                }
                prev = block;
            }
        }
        null_mut()
    }
}

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.lock().alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = round_size(&layout);
        let mut heap = self.0.lock();
        heap.used -= size;
        unsafe { heap.add_free(ptr as usize, size) }
    }
}

/// Reserva um quarto da memória livre (entre 1 e 32 MiB) para o heap; o
/// resto fica para as páginas dos processos.
pub fn init() {
    let (_, free) = memory::stats();
    let size = (free / 4).clamp(1 << 20, 32 << 20) / memory::FRAME_SIZE * memory::FRAME_SIZE;
    let base = memory::alloc_frames((size / memory::FRAME_SIZE) as usize).expect("sem memória para o heap");
    let mut heap = ALLOCATOR.0.lock();
    heap.size = size as usize;
    unsafe { heap.add_free(paging::phys_to_virt::<u8>(base) as usize, size as usize) }
}

/// (tamanho, em uso) em bytes.
pub fn stats() -> (usize, usize) {
    let heap = ALLOCATOR.0.lock();
    (heap.size, heap.used)
}
