//! Pipes: um buffer circular com uma ponta de leitura e outra de escrita.
//!
//! Quem lê de um pipe vazio dorme até alguém escrever; quem escreve num pipe
//! cheio dorme até alguém ler. Quando todas as pontas de escrita fecham, a
//! leitura devolve fim de arquivo; quando todas as de leitura fecham, quem
//! escreve recebe SIGPIPE (e EPIPE).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::signal::SIGPIPE;
use crate::sync::{SpinLock, WaitQueue};
use crate::syscall::{EAGAIN, EINTR, EPIPE, Errno};
use crate::task;

const CAPACITY: usize = 64 * 1024;

pub struct Pipe {
    buffer: SpinLock<VecDeque<u8>>,
    readers: AtomicUsize,
    writers: AtomicUsize,
    /// Acordada a cada leitura, escrita ou fechamento.
    queue: WaitQueue,
}

/// Uma ponta do pipe. Ao ser descartada (quando a última cópia do descritor
/// fecha), avisa a outra ponta.
pub struct PipeEnd {
    pipe: Arc<Pipe>,
    writer: bool,
}

pub fn new() -> (PipeEnd, PipeEnd) {
    let pipe = Arc::new(Pipe {
        buffer: SpinLock::new(VecDeque::new()),
        readers: AtomicUsize::new(1),
        writers: AtomicUsize::new(1),
        queue: WaitQueue::new(),
    });
    (PipeEnd { pipe: pipe.clone(), writer: false }, PipeEnd { pipe, writer: true })
}

impl PipeEnd {
    pub fn is_writer(&self) -> bool {
        self.writer
    }

    pub fn read(&self, max: usize, nonblocking: bool) -> Result<Vec<u8>, Errno> {
        let pipe = &self.pipe;
        loop {
            let seen = pipe.queue.generation();
            {
                let mut buffer = pipe.buffer.lock();
                if !buffer.is_empty() {
                    let count = max.min(buffer.len());
                    let data = buffer.drain(..count).collect();
                    pipe.queue.wake_all();
                    return Ok(data);
                }
            }
            if pipe.writers.load(Ordering::Acquire) == 0 {
                return Ok(Vec::new()); // fim de arquivo
            }
            if nonblocking {
                return Err(EAGAIN);
            }
            if task::signal_pending() {
                return Err(EINTR);
            }
            task::block_on(&pipe.queue, seen);
        }
    }

    pub fn write(&self, mut data: &[u8], nonblocking: bool) -> Result<usize, Errno> {
        let pipe = &self.pipe;
        let mut written = 0;
        loop {
            let seen = pipe.queue.generation();
            if pipe.readers.load(Ordering::Acquire) == 0 {
                task::send_signal_current(SIGPIPE);
                return if written > 0 { Ok(written) } else { Err(EPIPE) };
            }
            {
                let mut buffer = pipe.buffer.lock();
                let count = data.len().min(CAPACITY - buffer.len());
                if count > 0 {
                    buffer.extend(&data[..count]);
                    data = &data[count..];
                    written += count;
                    pipe.queue.wake_all();
                }
            }
            if data.is_empty() {
                return Ok(written);
            }
            if nonblocking || task::signal_pending() {
                return if written > 0 {
                    Ok(written)
                } else if nonblocking {
                    Err(EAGAIN)
                } else {
                    Err(EINTR)
                };
            }
            task::block_on(&pipe.queue, seen);
        }
    }
}

impl PipeEnd {
    /// Para `poll`: (dá para ler, dá para escrever, a outra ponta fechou).
    pub fn readiness(&self) -> (bool, bool, bool) {
        let used = self.pipe.buffer.lock().len();
        if self.writer {
            let closed = self.pipe.readers.load(Ordering::Acquire) == 0;
            (false, used < CAPACITY || closed, closed)
        } else {
            let closed = self.pipe.writers.load(Ordering::Acquire) == 0;
            (used > 0 || closed, false, closed)
        }
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let counter = if self.writer { &self.pipe.writers } else { &self.pipe.readers };
        counter.fetch_sub(1, Ordering::Release);
        self.pipe.queue.wake_all();
    }
}
