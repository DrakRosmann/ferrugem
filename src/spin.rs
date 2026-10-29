//! Um spinlock mínimo.
//!
//! Sem sistema operacional não existe `std::sync::Mutex` (ele depende do SO
//! para colocar threads para dormir). Num kernel, o jeito mais simples de
//! proteger um dado global é um *spinlock*: quem quer o lock fica girando
//! num laço até conseguir trocar `false` por `true` de forma atômica.

use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct Mutex<T> {
    travado: AtomicBool,
    dado: UnsafeCell<T>,
}

// Seguro compartilhar entre "threads" porque o acesso ao dado passa pelo lock.
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(dado: T) -> Self {
        Self { travado: AtomicBool::new(false), dado: UnsafeCell::new(dado) }
    }

    pub fn lock(&self) -> Guarda<'_, T> {
        while self
            .travado
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop(); // dica para a CPU (instrução PAUSE)
        }
        Guarda { mutex: self }
    }

    /// Solta o lock sem ter a `Guarda`. Só serve para o caminho de pânico:
    /// se o kernel quebrou no meio de um `print!`, o lock da tela ficou preso
    /// com quem quebrou, e precisamos dela para mostrar o erro.
    ///
    /// # Safety
    /// Quem segurava o lock não pode voltar a rodar.
    pub unsafe fn forcar_destravar(&self) {
        self.travado.store(false, Ordering::Release);
    }
}

/// Enquanto a `Guarda` existir, o lock está nosso. Ao sair de escopo, ele é liberado (RAII).
pub struct Guarda<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for Guarda<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.dado.get() }
    }
}

impl<T> DerefMut for Guarda<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.dado.get() }
    }
}

impl<T> Drop for Guarda<'_, T> {
    fn drop(&mut self) {
        self.mutex.travado.store(false, Ordering::Release);
    }
}
