//! `malloc` dos programas: uma lista livre que cresce pedindo memória ao
//! kernel com `brk` (que empurra o fim do heap do processo para cima).

use crate::lista_livre::ListaLivre;
use crate::sys;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;

struct Alocador {
    lista: UnsafeCell<ListaLivre>,
}

// Os programas têm uma thread só.
unsafe impl Sync for Alocador {}

#[global_allocator]
static ALOCADOR: Alocador = Alocador { lista: UnsafeCell::new(ListaLivre::new()) };

const PEDACO: usize = 64 * 1024;

unsafe impl GlobalAlloc for Alocador {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let lista = unsafe { &mut *self.lista.get() };
        let ptr = lista.alocar(layout);
        if !ptr.is_null() {
            return ptr;
        }
        // Sem espaço: pede mais ao kernel e tenta de novo.
        let pedido = (layout.size() + layout.align() + 32).next_multiple_of(PEDACO);
        let atual = sys::brk(0);
        if sys::brk(atual + pedido) < atual + pedido {
            return core::ptr::null_mut();
        }
        unsafe { lista.adicionar(atual, pedido) };
        lista.alocar(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { (*self.lista.get()).liberar(ptr, layout) }
    }
}
