//! Heap do kernel. Com ele passam a funcionar `Box`, `Vec`, `String`, `Arc`,
//! `BTreeMap`... (a crate `alloc` do Rust só precisa de um `#[global_allocator]`).
//!
//! A área é um array estático no .bss: simples, e já mapeado em identidade.

use crate::interrupcoes::sem_interrupcoes;
use crate::lista_livre::ListaLivre;
use crate::spin::Mutex;
use core::alloc::{GlobalAlloc, Layout};

pub const TAMANHO: usize = 32 * 1024 * 1024;

#[repr(C, align(4096))]
struct Area([u8; TAMANHO]);

static mut AREA: Area = Area([0; TAMANHO]);

struct Heap {
    lista: Mutex<ListaLivre>,
}

#[global_allocator]
static HEAP: Heap = Heap { lista: Mutex::new(ListaLivre::new()) };

pub fn init() {
    sem_interrupcoes(|| unsafe { HEAP.lista.lock().adicionar((&raw mut AREA) as usize, TAMANHO) });
}

/// Bytes em uso no heap.
pub fn usado() -> usize {
    TAMANHO - sem_interrupcoes(|| HEAP.lista.lock().livre())
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        sem_interrupcoes(|| self.lista.lock().alocar(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        sem_interrupcoes(|| unsafe { self.lista.lock().liberar(ptr, layout) })
    }
}
