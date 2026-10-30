//! A placa de rede RTL8139 (Realtek), que o QEMU emula (`-device rtl8139`).
//!
//! Uma das placas mais simples de programar, toda por portas de E/S:
//!
//! - **Receber:** damos à placa um buffer circular de 8 KiB na RAM; ela
//!   escreve sozinha (DMA) cada quadro que chega — um cabeçalho de 4 bytes
//!   (estado e tamanho) e os dados — e avisa pela IRQ. Nós lemos a partir
//!   de onde paramos e dizemos até onde já lemos (CAPR).
//! - **Enviar:** há 4 "descritores": para cada um, dizemos onde está o
//!   quadro na RAM (TSAD) e o tamanho (TSD); a placa envia e marca como
//!   pronto. Usamos os 4 em rodízio.

use crate::porta::{inb, inl, inw, outb, outl, outw};
use crate::spin::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

const TAMANHO_RX: usize = 8192;
const ROK: u16 = 1 << 0; // um quadro chegou

struct Placa {
    io: u16,
    /// O buffer de recepção (com folga: no modo WRAP o quadro pode passar do fim).
    rx: Vec<u64>,
    posicao_rx: usize,
    tx: [Vec<u64>; 4],
    proximo_tx: usize,
    mac: [u8; 6],
}

static PLACA: Mutex<Option<Placa>> = Mutex::new(None);
static IRQ: AtomicU8 = AtomicU8::new(0xFF);

/// O endereço físico de um buffer (o heap do kernel tem endereço virtual = físico).
fn fisico(buf: &[u64]) -> u32 {
    buf.as_ptr() as u32
}

pub fn init() -> Option<String> {
    let pci = crate::pci::achar(0x10EC, 0x8139)?;
    crate::pci::habilitar(pci);
    let io = (crate::pci::ler(pci, 0x10) & !0x3) as u16;
    let irq = crate::pci::irq(pci);
    unsafe {
        outb(io + 0x52, 0x00); // liga a placa
        outb(io + 0x37, 0x10); // reinicia...
        while inb(io + 0x37) & 0x10 != 0 {} // ...e espera terminar
    }
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        *b = unsafe { inb(io + i as u16) };
    }
    let placa = Placa {
        io,
        rx: vec![0u64; (TAMANHO_RX + 16 + 1536) / 8],
        posicao_rx: 0,
        tx: [vec![0u64; 1536 / 8], vec![0u64; 1536 / 8], vec![0u64; 1536 / 8], vec![0u64; 1536 / 8]],
        proximo_tx: 0,
        mac,
    };
    unsafe {
        outl(io + 0x30, fisico(&placa.rx)); // RBSTART: o buffer de recepção
        outw(io + 0x3C, ROK); // IMR: só a chegada de quadros gera IRQ
        // RCR: aceitar quadros para nós, de broadcast e de multicast; WRAP
        outl(io + 0x44, 0xF | 1 << 7);
        outb(io + 0x37, 0x0C); // CR: liga recepção e envio
    }
    *PLACA.lock() = Some(placa);
    IRQ.store(irq, Ordering::Relaxed);
    crate::pic::liberar(irq);
    Some(format!(
        "rede: RTL8139 na IRQ {}, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        irq, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    ))
}

pub fn irq() -> u8 {
    IRQ.load(Ordering::Relaxed)
}

pub fn mac() -> Option<[u8; 6]> {
    PLACA.lock().as_ref().map(|p| p.mac)
}

/// Envia um quadro Ethernet (de 14 a 1514 bytes).
pub fn enviar(quadro: &[u8]) -> bool {
    let mut guarda = PLACA.lock();
    let Some(p) = guarda.as_mut() else { return false };
    if quadro.len() > 1514 {
        return false;
    }
    let i = p.proximo_tx;
    let tsd = p.io + 0x10 + i as u16 * 4;
    // O descritor ainda está enviando o quadro anterior? (bit OWN = a placa já devolveu)
    for _ in 0..100_000 {
        let estado = unsafe { inl(tsd) };
        if estado & (1 << 13) != 0 || estado == 0x2000 || estado & 0x1FFF == 0 {
            break;
        }
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(p.tx[i].as_mut_ptr() as *mut u8, 1536) };
    buf[..quadro.len()].copy_from_slice(quadro);
    let tamanho = quadro.len().max(60); // o mínimo do Ethernet (a placa completa com zeros)
    buf[quadro.len()..tamanho].fill(0);
    unsafe {
        outl(p.io + 0x20 + i as u16 * 4, fisico(&p.tx[i])); // TSAD
        outl(tsd, tamanho as u32); // TSD: o tamanho (e OWN = 0: "pode enviar")
    }
    p.proximo_tx = (i + 1) % 4;
    true
}

/// A IRQ da placa: lê os quadros que chegaram e os entrega à pilha de rede.
pub fn tratar_irq() {
    let mut quadros = Vec::new();
    {
        let mut guarda = PLACA.lock();
        let Some(p) = guarda.as_mut() else { return };
        let isr = unsafe { inw(p.io + 0x3E) };
        unsafe { outw(p.io + 0x3E, isr) }; // "visto"
        if isr & ROK != 0 {
            let rx = unsafe { core::slice::from_raw_parts(p.rx.as_ptr() as *const u8, TAMANHO_RX + 16 + 1536) };
            // Enquanto o buffer não estiver vazio (bit BUFE do CR)...
            while unsafe { inb(p.io + 0x37) } & 1 == 0 {
                let o = p.posicao_rx;
                let estado = u16::from_le_bytes([rx[o], rx[o + 1]]);
                let tamanho = u16::from_le_bytes([rx[o + 2], rx[o + 3]]) as usize;
                if estado & 1 == 0 || !(64..=1518).contains(&tamanho) {
                    break; // quadro com erro: deixa para a próxima
                }
                quadros.push(rx[o + 4..o + 4 + tamanho - 4].to_vec()); // sem o CRC do fim
                p.posicao_rx = (o + 4 + tamanho + 3) & !3;
                if p.posicao_rx >= TAMANHO_RX {
                    p.posicao_rx -= TAMANHO_RX;
                }
                unsafe { outw(p.io + 0x38, (p.posicao_rx as u16).wrapping_sub(16)) }; // CAPR
            }
        }
    }
    for q in quadros {
        crate::rede::receber_quadro(&q);
    }
}
