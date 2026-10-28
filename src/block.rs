//! Dispositivos de bloco e o driver ATA (IDE) em modo PIO.
//!
//! O ATA PIO é o jeito mais simples de ler um disco num PC: o kernel
//! escreve o endereço (LBA) e o comando em portas de E/S e copia cada setor
//! de 512 bytes pela porta de dados. Ele usa a CPU o tempo todo (sem DMA)
//! e não usa interrupções (o kernel espera o disco consultando o status),
//! mas funciona em qualquer PC e no QEMU (`-drive ...,if=ide`).

use alloc::string::String;
use core::arch::asm;

use crate::cpu::{inb, outb};
use crate::sync::SpinLock;
use crate::syscall::{EIO, Errno};

pub const SECTOR_SIZE: usize = 512;

pub trait BlockDevice: Send + Sync {
    /// Lê `buf.len() / 512` setores a partir de `lba`.
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), Errno>;
    /// Grava setores inteiros a partir de `lba`, e só volta quando estão no disco.
    fn write(&self, lba: u64, data: &[u8]) -> Result<(), Errno>;
    fn sectors(&self) -> u64;
    fn name(&self) -> &str;
}

// Registradores do canal primário.
const DATA: u16 = 0x1F0;
const SECTOR_COUNT: u16 = 0x1F2;
const LBA_LOW: u16 = 0x1F3;
const LBA_MID: u16 = 0x1F4;
const LBA_HIGH: u16 = 0x1F5;
const DRIVE: u16 = 0x1F6;
const STATUS_COMMAND: u16 = 0x1F7;
const CONTROL: u16 = 0x3F6;

const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_DF: u8 = 0x20;
const STATUS_BSY: u8 = 0x80;

const CMD_READ_EXT: u8 = 0x24;
const CMD_WRITE_EXT: u8 = 0x34;
const CMD_FLUSH_EXT: u8 = 0xEA;
const CMD_IDENTIFY: u8 = 0xEC;

/// Um disco ATA no canal primário (mestre ou escravo).
pub struct AtaDisk {
    /// 0xE0 (mestre) ou 0xF0 (escravo), com o bit de LBA ligado.
    select: u8,
    sectors: u64,
    model: String,
    /// Um comando por vez no controlador.
    lock: SpinLock<()>,
}

fn status() -> u8 {
    unsafe { inb(STATUS_COMMAND) }
}

/// Os ~400 ns que o controlador pede depois de trocar de drive.
fn delay() {
    for _ in 0..4 {
        unsafe { inb(CONTROL) };
    }
}

/// Espera o disco ficar livre e, com `data`, pronto para transferir.
fn wait(data: bool) -> Result<(), Errno> {
    for _ in 0..10_000_000 {
        let status = status();
        if status & STATUS_BSY != 0 {
            core::hint::spin_loop();
            continue;
        }
        if status & (STATUS_ERR | STATUS_DF) != 0 {
            return Err(EIO);
        }
        if !data || status & STATUS_DRQ != 0 {
            return Ok(());
        }
    }
    Err(EIO)
}

impl AtaDisk {
    /// Procura um disco ATA (não um CD-ROM) no canal primário.
    pub fn probe(slave: bool) -> Option<AtaDisk> {
        let select = if slave { 0xF0 } else { 0xE0 };
        unsafe {
            if inb(STATUS_COMMAND) == 0xFF {
                return None; // barramento flutuando: não há controlador
            }
            outb(CONTROL, 0x02); // nIEN: sem interrupções, só consulta ao status
            outb(DRIVE, select);
            delay();
            for port in [SECTOR_COUNT, LBA_LOW, LBA_MID, LBA_HIGH] {
                outb(port, 0);
            }
            outb(STATUS_COMMAND, CMD_IDENTIFY);
            if status() == 0 {
                return None; // nenhum drive
            }
            while status() & STATUS_BSY != 0 {}
            if inb(LBA_MID) != 0 || inb(LBA_HIGH) != 0 {
                return None; // ATAPI (CD-ROM) ou SATA: não é um disco ATA
            }
        }
        wait(true).ok()?;
        let mut identify = [0u16; 256];
        unsafe {
            asm!("rep insw", in("dx") DATA, inout("rdi") identify.as_mut_ptr() => _,
                 inout("rcx") 256usize => _, options(nostack, preserves_flags));
        }
        if identify[83] & (1 << 10) == 0 {
            return None; // sem LBA48 (discos de antes de 2003)
        }
        let sectors = (0..4).fold(0u64, |acc, i| acc | (identify[100 + i] as u64) << (16 * i));
        // O modelo vem em palavras com os bytes trocados (27..46).
        let model: String = identify[27..47]
            .iter()
            .flat_map(|word| [(word >> 8) as u8 as char, *word as u8 as char])
            .collect();
        Some(AtaDisk { select, sectors, model: String::from(model.trim()), lock: SpinLock::new(()) })
    }

    /// Prepara um comando LBA48 para `count` setores a partir de `lba`.
    fn command(&self, lba: u64, count: u16, command: u8) -> Result<(), Errno> {
        if lba + count as u64 > self.sectors || count == 0 {
            return Err(EIO);
        }
        unsafe {
            outb(DRIVE, self.select & 0xF0 | 0x40);
            delay();
            wait(false)?;
            // Primeiro os bytes altos, depois os baixos (registradores em FIFO).
            outb(SECTOR_COUNT, (count >> 8) as u8);
            outb(LBA_LOW, (lba >> 24) as u8);
            outb(LBA_MID, (lba >> 32) as u8);
            outb(LBA_HIGH, (lba >> 40) as u8);
            outb(SECTOR_COUNT, count as u8);
            outb(LBA_LOW, lba as u8);
            outb(LBA_MID, (lba >> 8) as u8);
            outb(LBA_HIGH, (lba >> 16) as u8);
            outb(STATUS_COMMAND, command);
        }
        Ok(())
    }
}

impl BlockDevice for AtaDisk {
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), Errno> {
        let _guard = self.lock.lock();
        for (i, sector) in buf.chunks_exact_mut(SECTOR_SIZE).enumerate() {
            // Um setor por comando: simples, e o QEMU é rápido nisso.
            self.command(lba + i as u64, 1, CMD_READ_EXT)?;
            wait(true)?;
            unsafe {
                asm!("rep insw", in("dx") DATA, inout("rdi") sector.as_mut_ptr() => _,
                     inout("rcx") SECTOR_SIZE / 2 => _, options(nostack, preserves_flags));
            }
        }
        Ok(())
    }

    fn write(&self, lba: u64, data: &[u8]) -> Result<(), Errno> {
        let _guard = self.lock.lock();
        for (i, sector) in data.chunks_exact(SECTOR_SIZE).enumerate() {
            self.command(lba + i as u64, 1, CMD_WRITE_EXT)?;
            wait(true)?;
            for word in sector.chunks_exact(2) {
                // `rep outsw` rápido demais pode atropelar alguns controladores.
                unsafe { asm!("out dx, ax", in("dx") DATA, in("ax") u16::from_le_bytes([word[0], word[1]]),
                              options(nomem, nostack, preserves_flags)) };
            }
            wait(false)?;
        }
        unsafe { outb(STATUS_COMMAND, CMD_FLUSH_EXT) };
        wait(false)
    }

    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn name(&self) -> &str {
        &self.model
    }
}
