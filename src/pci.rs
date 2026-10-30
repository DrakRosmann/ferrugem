//! O barramento PCI: como achar os dispositivos e configurá-los.
//!
//! Cada dispositivo PCI tem um "espaço de configuração" de 256 bytes: quem
//! fez (vendedor), o que é (dispositivo), onde estão os registradores dele
//! (as BARs: portas de E/S ou memória) e qual IRQ ele usa. Lemos e escrevemos
//! esse espaço pelas portas 0xCF8 (o endereço) e 0xCFC (o dado).

use crate::porta::{inl, outl};

/// Um dispositivo: (barramento, dispositivo, função).
#[derive(Clone, Copy)]
pub struct Endereco(pub u32, pub u32, pub u32);

fn selecionar(e: Endereco, registro: u32) {
    let Endereco(b, d, f) = e;
    unsafe { outl(0xCF8, 1 << 31 | b << 16 | d << 11 | f << 8 | registro & 0xFC) };
}

pub fn ler(e: Endereco, registro: u32) -> u32 {
    selecionar(e, registro);
    unsafe { inl(0xCFC) }
}

#[allow(dead_code)]
pub fn escrever(e: Endereco, registro: u32, valor: u32) {
    selecionar(e, registro);
    unsafe { outl(0xCFC, valor) };
}

/// Procura no barramento 0 um dispositivo (vendedor, dispositivo).
pub fn achar(vendedor: u16, dispositivo: u16) -> Option<Endereco> {
    let id = (dispositivo as u32) << 16 | vendedor as u32;
    (0..32).map(|d| Endereco(0, d, 0)).find(|&e| ler(e, 0) == id)
}

/// Liga o acesso às portas de E/S e à memória, e o "bus master" (o
/// dispositivo pode ler e escrever na RAM sozinho: DMA).
pub fn habilitar(e: Endereco) {
    let comando = ler(e, 0x04);
    escrever(e, 0x04, comando | 0b111);
}

/// A IRQ que o dispositivo usa (a BIOS preenche no registro 0x3C).
pub fn irq(e: Endereco) -> u8 {
    ler(e, 0x3C) as u8
}
