//! Trava giratória que também desabilita interrupções enquanto está presa.
//!
//! Com um único núcleo, isso garante que um tratador de interrupção nunca
//! tente pegar uma trava que o código interrompido já segura.

use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::cpu;

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), data: UnsafeCell::new(value) }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        let irq_was_enabled = cpu::interrupts_enabled();
        cpu::cli();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
        SpinGuard { lock: self, irq_was_enabled }
    }

    /// Solta a trava à força. Só para o tratador de pânico, que precisa
    /// escrever na tela mesmo que o código que entrou em pânico a segurasse.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
    irq_was_enabled: bool,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.irq_was_enabled {
            cpu::sti();
        }
    }
}

/// Ponto de espera: quem espera anota a "geração" atual antes de testar sua
/// condição e dorme até ela mudar; quem acorda só incrementa o contador.
/// Acordar não pega nenhuma trava, então pode ser feito de qualquer lugar,
/// inclusive de um tratador de interrupção ou do `Drop` de um pipe.
pub struct WaitQueue(core::sync::atomic::AtomicU64);

impl WaitQueue {
    pub const fn new() -> Self {
        Self(core::sync::atomic::AtomicU64::new(0))
    }

    pub fn generation(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    pub fn wake_all(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}
