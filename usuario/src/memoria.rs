//! `malloc` dos programas. Pedidos pequenos (até 1 KiB) vêm de listas por
//! tamanho: alocar e liberar é só tirar e pôr na cabeça da lista, o que faz
//! muita diferença para quem cria milhões de objetos pequenos (o JavaScript
//! do navegador). Os grandes usam a lista livre, que cresce pedindo memória
//! ao kernel com `brk` (que empurra o fim do heap do processo para cima).

use crate::lista_livre::ListaLivre;
use crate::sys;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;

const GRANULO: usize = 16;
const MAIOR_PEQUENO: usize = 1024;
const N_CLASSES: usize = MAIOR_PEQUENO / GRANULO;
/// Quanto tirar de uma vez da lista grande para os pequenos.
const FATIA: usize = 256 * 1024;

struct Estado {
    lista: ListaLivre,
    /// A cabeça da lista de blocos livres de cada tamanho.
    classes: [*mut u8; N_CLASSES],
    /// Uma fatia sendo cortada em blocos pequenos.
    corte: usize,
    corte_fim: usize,
}

struct Alocador {
    e: UnsafeCell<Estado>,
}

// Os programas têm uma thread só.
unsafe impl Sync for Alocador {}

#[global_allocator]
static ALOCADOR: Alocador =
    Alocador { e: UnsafeCell::new(Estado { lista: ListaLivre::new(), classes: [null_mut(); N_CLASSES], corte: 0, corte_fim: 0 }) };

const PEDACO: usize = 64 * 1024;

impl Estado {
    fn grande(&mut self, layout: Layout) -> *mut u8 {
        let ptr = self.lista.alocar(layout);
        if !ptr.is_null() {
            return ptr;
        }
        // Sem espaço: pede mais ao kernel e tenta de novo.
        let pedido = (layout.size() + layout.align() + 32).next_multiple_of(PEDACO);
        let atual = sys::brk(0);
        if sys::brk(atual + pedido) < atual + pedido {
            return null_mut();
        }
        unsafe { self.lista.adicionar(atual, pedido) };
        self.lista.alocar(layout)
    }

    fn pequeno(&mut self, classe: usize) -> *mut u8 {
        let cabeca = self.classes[classe];
        if !cabeca.is_null() {
            self.classes[classe] = unsafe { *(cabeca as *mut *mut u8) };
            return cabeca;
        }
        let tam = (classe + 1) * GRANULO;
        if self.corte + tam > self.corte_fim {
            let Ok(l) = Layout::from_size_align(FATIA, GRANULO) else { return null_mut() };
            let p = self.grande(l);
            if p.is_null() {
                return null_mut();
            }
            self.corte = p as usize;
            self.corte_fim = self.corte + FATIA;
        }
        let p = self.corte;
        self.corte += tam;
        p as *mut u8
    }
}

fn classe_de(layout: &Layout) -> Option<usize> {
    if layout.size() <= MAIOR_PEQUENO && layout.align() <= GRANULO {
        Some(layout.size().max(1).div_ceil(GRANULO) - 1)
    } else {
        None
    }
}

unsafe impl GlobalAlloc for Alocador {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let e = unsafe { &mut *self.e.get() };
        match classe_de(&layout) {
            Some(c) => e.pequeno(c),
            None => e.grande(layout),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let e = unsafe { &mut *self.e.get() };
        match classe_de(&layout) {
            Some(c) => unsafe {
                *(ptr as *mut *mut u8) = e.classes[c];
                e.classes[c] = ptr;
            },
            None => unsafe { e.lista.liberar(ptr, layout) },
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, novo: usize) -> *mut u8 {
        // Mesmo bloco, se a classe não muda.
        let Ok(nl) = Layout::from_size_align(novo, layout.align()) else { return null_mut() };
        if let (Some(a), Some(b)) = (classe_de(&layout), classe_de(&nl)) {
            if a == b {
                return ptr;
            }
        }
        let p = unsafe { self.alloc(nl) };
        if !p.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, p, layout.size().min(novo));
                self.dealloc(ptr, layout);
            }
        }
        p
    }
}
