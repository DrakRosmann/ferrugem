//! Fila de bytes de entrada, alimentada pelo teclado e pela porta serial.
//!
//! Se a fila enche (um texto longo colado mais rápido do que é lido), a
//! serial para de ser esvaziada e os bytes esperam na própria UART em vez
//! de se perderem; ao abrir espaço, a recepção volta.

use crate::sync::{SpinLock, WaitQueue};
use crate::{serial, task, tty};

/// Como o buffer de leitura do terminal no Linux (N_TTY).
const SIZE: usize = 4096;

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

/// Cabe mais um byte?
pub fn has_room() -> bool {
    let ring = INPUT.lock();
    (ring.head + 1) % SIZE != ring.tail
}

pub fn push_all(bytes: &[u8]) {
    for &byte in bytes {
        push(byte);
    }
}

pub fn pop() -> Option<u8> {
    let byte = {
        let mut ring = INPUT.lock();
        if ring.head == ring.tail {
            return None;
        }
        let byte = ring.buf[ring.tail];
        ring.tail = (ring.tail + 1) % SIZE;
        byte
    };
    serial::resume_rx();
    Some(byte)
}

/// Descarta o que foi digitado e ainda não foi lido.
pub fn flush() {
    {
        let mut ring = INPUT.lock();
        ring.tail = ring.head;
    }
    serial::resume_rx();
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
