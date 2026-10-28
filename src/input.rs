//! Fila de bytes de entrada, alimentada pelo teclado e pela porta serial.

use crate::sync::{SpinLock, WaitQueue};
use crate::{task, tty};

const SIZE: usize = 256;

struct Ring {
    buf: [u8; SIZE],
    head: usize,
    tail: usize,
}

static INPUT: SpinLock<Ring> = SpinLock::new(Ring { buf: [0; SIZE], head: 0, tail: 0 });

/// Acordada a cada byte que chega.
pub static QUEUE: WaitQueue = WaitQueue::new();

/// Chamado pelos tratadores de interrupção do teclado e da serial.
pub fn push(byte: u8) {
    if tty::intercept(byte) {
        return;
    }
    {
        let mut ring = INPUT.lock();
        let next = (ring.head + 1) % SIZE;
        if next != ring.tail {
            let head = ring.head;
            ring.buf[head] = byte;
            ring.head = next;
        }
    }
    QUEUE.wake_all();
}

pub fn push_all(bytes: &[u8]) {
    for &byte in bytes {
        push(byte);
    }
}

pub fn pop() -> Option<u8> {
    let mut ring = INPUT.lock();
    if ring.head == ring.tail {
        return None;
    }
    let byte = ring.buf[ring.tail];
    ring.tail = (ring.tail + 1) % SIZE;
    Some(byte)
}

/// Descarta o que foi digitado e ainda não foi lido.
pub fn flush() {
    let mut ring = INPUT.lock();
    ring.tail = ring.head;
}

/// Espera um byte (usado pelo monitor do kernel).
pub fn read_byte() -> u8 {
    loop {
        let seen = QUEUE.generation();
        if let Some(byte) = pop() {
            return byte;
        }
        task::block_on(&QUEUE, seen);
    }
}
