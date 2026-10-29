//! Mouse PS/2 (IRQ 12).
//!
//! O mouse fica pendurado no mesmo controlador 8042 do teclado, na "porta
//! auxiliar": para falar com ele, mandamos 0xD4 ao controlador antes de cada
//! byte. Depois de ligado, a cada movimento ou clique ele manda um pacote de
//! 3 bytes:
//!
//! ```text
//!   byte 0: bits 0-2 = botões (esquerdo, direito, meio), bit 3 = sempre 1,
//!           bits 4-5 = sinal de dx e dy
//!   byte 1: dx (quanto andou para a direita)
//!   byte 2: dy (quanto andou para CIMA)
//!   byte 3: a roda (só no modo IntelliMouse): -1 = para cima, +1 = para baixo
//! ```
//!
//! O modo IntelliMouse (com a roda) é ligado por uma "senha": mudar a taxa de
//! amostragem para 200, 100 e 80, nessa ordem. Se o mouse passar a dizer que
//! é do tipo 3, os pacotes ganham o 4º byte.
//!
//! `/dev/mouse` entrega sempre pacotes de 4 bytes (o da roda é 0 num mouse
//! sem roda).

use crate::erro::*;
use crate::porta::{inb, outb};
use crate::processo;
use crate::spin::Mutex;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicUsize, Ordering};

const DADOS: u16 = 0x60;
const STATUS: u16 = 0x64;

struct Estado {
    pacote: [u8; 4],
    recebidos: usize,
    /// 3 bytes por pacote, ou 4 com a roda.
    tamanho: usize,
    fila: VecDeque<u8>,
}

static ESTADO: Mutex<Estado> =
    Mutex::new(Estado { pacote: [0; 4], recebidos: 0, tamanho: 3, fila: VecDeque::new() });
static LEITORES: AtomicUsize = AtomicUsize::new(0);

fn canal() -> usize {
    &raw const ESTADO as usize
}

fn esperar_para_escrever() {
    for _ in 0..100_000 {
        if unsafe { inb(STATUS) } & 2 == 0 {
            return;
        }
    }
}

fn esperar_resposta() -> Option<u8> {
    for _ in 0..100_000 {
        if unsafe { inb(STATUS) } & 1 != 0 {
            return Some(unsafe { inb(DADOS) });
        }
    }
    None
}

fn mandar_ao_mouse(byte: u8) -> Option<u8> {
    esperar_para_escrever();
    unsafe { outb(STATUS, 0xD4) };
    esperar_para_escrever();
    unsafe { outb(DADOS, byte) };
    esperar_resposta() // 0xFA = entendido
}

/// Liga a porta auxiliar, a IRQ 12 e o envio de pacotes. Devolve se achou um mouse.
pub fn init() -> bool {
    unsafe {
        esperar_para_escrever();
        outb(STATUS, 0xA8); // liga a porta auxiliar
        esperar_para_escrever();
        outb(STATUS, 0x20); // lê o byte de configuração do controlador
        let Some(config) = esperar_resposta() else { return false };
        esperar_para_escrever();
        outb(STATUS, 0x60); // ...e o grava com a IRQ 12 ligada e o relógio do mouse ativo
        esperar_para_escrever();
        outb(DADOS, (config | 0x02) & !0x20);
    }
    let mut ok = mandar_ao_mouse(0xF6) == Some(0xFA); // configuração padrão
    // A "senha" do IntelliMouse; depois, quem é você? (0xF2: o tipo)
    for taxa in [200, 100, 80] {
        mandar_ao_mouse(0xF3);
        mandar_ao_mouse(taxa);
    }
    if mandar_ao_mouse(0xF2) == Some(0xFA) && esperar_resposta() == Some(3) {
        ESTADO.lock().tamanho = 4;
    }
    ok &= mandar_ao_mouse(0xF4) == Some(0xFA); // começa a mandar pacotes
    // descarta o que tenha sobrado
    while unsafe { inb(STATUS) } & 1 != 0 {
        unsafe { inb(DADOS) };
    }
    ok
}

/// Um byte do mouse (da IRQ 12, ou da IRQ 1 quando o byte era do mouse).
pub fn receber(byte: u8) {
    let mut e = ESTADO.lock();
    // O byte 0 sempre tem o bit 3 ligado: se não tiver, perdemos a sincronia.
    // Um 0xFA ali é a confirmação ("entendido") de um comando que chegou
    // atrasada; os bits 6 e 7 (estouro) também nunca vêm num pacote útil.
    if e.recebidos == 0 && (byte & 0x08 == 0 || byte == 0xFA || byte & 0xC0 != 0) {
        return;
    }
    let i = e.recebidos;
    e.pacote[i] = byte;
    e.recebidos += 1;
    if e.recebidos == e.tamanho {
        e.recebidos = 0;
        if LEITORES.load(Ordering::Relaxed) > 0 && e.fila.len() < 4 * 128 {
            let mut pacote = e.pacote;
            if e.tamanho == 3 {
                pacote[3] = 0;
            }
            e.fila.extend(pacote);
            drop(e);
            processo::acordar(canal());
        }
    }
}

pub fn tratar_irq() {
    let byte = unsafe { inb(DADOS) };
    receber(byte);
}

pub fn abrir() {
    LEITORES.fetch_add(1, Ordering::Relaxed);
}

pub fn fechar() {
    if LEITORES.fetch_sub(1, Ordering::Relaxed) == 1 {
        ESTADO.lock().fila.clear();
    }
}

/// Entrega pacotes inteiros (múltiplos de 4 bytes).
pub fn ler(buf: &mut [u8], bloquear: bool) -> Result<usize, i64> {
    loop {
        {
            let mut e = ESTADO.lock();
            let n = (buf.len() / 4 * 4).min(e.fila.len());
            if n > 0 {
                for (destino, b) in buf.iter_mut().zip(e.fila.drain(..n)) {
                    *destino = b;
                }
                return Ok(n);
            }
        }
        if !bloquear {
            return Err(EAGAIN);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(canal());
    }
}

pub fn tem_roda() -> bool {
    ESTADO.lock().tamanho == 4
}

pub fn pode_ler() -> bool {
    !ESTADO.lock().fila.is_empty()
}
