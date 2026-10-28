//! Alocador de memória dos programas.
//!
//! Blocos pequenos (até 64 KiB) são arredondados para uma potência de 2 e
//! saem de listas livres por tamanho; quando uma lista está vazia, o heap
//! cresce com `brk`. Blocos liberados voltam para a lista do seu tamanho.
//! Blocos grandes vêm direto do kernel com `mmap` e voltam com `munmap`.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;

use crate::sys;

const MIN_CLASS: u32 = 4; // 16 bytes
const MAX_CLASS: u32 = 16; // 64 KiB
const CLASSES: usize = (MAX_CLASS - MIN_CLASS + 1) as usize;
const GROW_STEP: u64 = 64 * 1024;

struct Allocator {
    /// Cabeça de cada lista livre (o próximo fica nos 8 primeiros bytes).
    free: [Cell<u64>; CLASSES],
    next: Cell<u64>,
    end: Cell<u64>,
}

// Os programas têm uma única thread.
unsafe impl Sync for Allocator {}

#[global_allocator]
static ALLOCATOR: Allocator =
    Allocator { free: [const { Cell::new(0) }; CLASSES], next: Cell::new(0), end: Cell::new(0) };

/// Classe de tamanho de um layout, ou `None` se ele for grande.
fn class(layout: &Layout) -> Option<usize> {
    let size = layout.size().max(layout.align()).max(1 << MIN_CLASS).next_power_of_two();
    let bits = size.trailing_zeros();
    (bits <= MAX_CLASS).then(|| (bits - MIN_CLASS) as usize)
}

impl Allocator {
    /// Fatia nova do fim do heap, alinhada ao próprio tamanho.
    fn carve(&self, size: u64) -> *mut u8 {
        if self.end.get() == 0 {
            let start = sys::brk(0);
            self.next.set(start);
            self.end.set(start);
        }
        let start = self.next.get().next_multiple_of(size);
        let end = start + size;
        if end > self.end.get() {
            let wanted = end.next_multiple_of(GROW_STEP);
            if sys::brk(wanted) < end {
                return core::ptr::null_mut();
            }
            self.end.set(wanted);
        }
        self.next.set(end);
        start as *mut u8
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(class) = class(&layout) else {
            return sys::mmap_anonymous(layout.size()).unwrap_or(core::ptr::null_mut());
        };
        let head = &self.free[class];
        if head.get() != 0 {
            let block = head.get() as *mut u64;
            head.set(unsafe { block.read() });
            return block as *mut u8;
        }
        self.carve(1 << (class as u32 + MIN_CLASS))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match class(&layout) {
            Some(class) => {
                let head = &self.free[class];
                unsafe { (ptr as *mut u64).write(head.get()) };
                head.set(ptr as u64);
            }
            None => {
                sys::munmap(ptr, layout.size());
            }
        }
    }
}
