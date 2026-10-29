//! Disco ATA (IDE) em modo PIO: o jeito mais simples de ler e gravar um disco.
//!
//! O controlador do barramento primário fica nas portas 0x1F0..0x1F7. Para
//! ler um setor (512 bytes): escolhemos o disco e o endereço (LBA), mandamos
//! o comando, esperamos o bit DRQ ("dados prontos") e lemos 256 palavras de
//! 16 bits da porta de dados. Gravar é o mesmo caminho ao contrário.
//!
//! "PIO" = a própria CPU copia cada palavra. É lento comparado a DMA, mas
//! não precisa de nada além de `in`/`out`. Usamos *polling* (consultamos o
//! status em laço) em vez da IRQ 14: mais simples, e o kernel não é
//! preemptivo mesmo.

use crate::erro::EIO;
use crate::porta::{inb, inw, outb, outw};

const DADOS: u16 = 0x1F0;
const CONTAGEM: u16 = 0x1F2;
const LBA_BAIXO: u16 = 0x1F3;
const LBA_MEIO: u16 = 0x1F4;
const LBA_ALTO: u16 = 0x1F5;
const DISCO: u16 = 0x1F6;
const COMANDO: u16 = 0x1F7; // escrevendo: comando; lendo: status
const CONTROLE: u16 = 0x3F6;

const OCUPADO: u8 = 0x80;
const FALHA: u8 = 0x20;
const DADOS_PRONTOS: u8 = 0x08;
const ERRO: u8 = 0x01;

const LER_SETORES: u8 = 0x20;
const GRAVAR_SETORES: u8 = 0x30;
const ESVAZIAR_CACHE: u8 = 0xE7;
const IDENTIFICAR: u8 = 0xEC;

/// Ler o status 4 vezes dá os ~400 ns que o disco pede depois de trocar de unidade.
fn pausa() {
    for _ in 0..4 {
        unsafe { inb(COMANDO) };
    }
}

fn esperar_livre() -> Result<u8, i64> {
    for _ in 0..10_000_000 {
        let status = unsafe { inb(COMANDO) };
        if status & OCUPADO == 0 {
            return Ok(status);
        }
    }
    Err(EIO)
}

fn esperar_dados() -> Result<(), i64> {
    loop {
        let status = esperar_livre()?;
        if status & (ERRO | FALHA) != 0 {
            return Err(EIO);
        }
        if status & DADOS_PRONTOS != 0 {
            return Ok(());
        }
    }
}

/// Procura o disco mestre do barramento primário. Devolve quantos setores ele tem.
pub fn init() -> Option<u64> {
    unsafe {
        outb(CONTROLE, 0x02); // nIEN: o disco não gera interrupções
        outb(DISCO, 0xA0);
        pausa();
        outb(CONTAGEM, 0);
        outb(LBA_BAIXO, 0);
        outb(LBA_MEIO, 0);
        outb(LBA_ALTO, 0);
        outb(COMANDO, IDENTIFICAR);
        if inb(COMANDO) == 0 {
            return None; // ninguém respondeu: não há disco
        }
        esperar_livre().ok()?;
        if inb(LBA_MEIO) != 0 || inb(LBA_ALTO) != 0 {
            return None; // é um CD (ATAPI), não um disco ATA
        }
        esperar_dados().ok()?;
        let mut identidade = [0u16; 256];
        for palavra in identidade.iter_mut() {
            *palavra = inw(DADOS);
        }
        Some(identidade[60] as u64 | (identidade[61] as u64) << 16)
    }
}

fn preparar(lba: u32, setores: usize, comando: u8) {
    unsafe {
        outb(DISCO, 0xE0 | ((lba >> 24) & 0x0F) as u8); // modo LBA, mestre
        outb(CONTAGEM, setores as u8);
        outb(LBA_BAIXO, lba as u8);
        outb(LBA_MEIO, (lba >> 8) as u8);
        outb(LBA_ALTO, (lba >> 16) as u8);
        outb(COMANDO, comando);
    }
}

/// Lê `buf.len() / 512` setores (no máximo 255) a partir de `lba`.
pub fn ler(lba: u32, buf: &mut [u8]) -> Result<(), i64> {
    preparar(lba, buf.len() / 512, LER_SETORES);
    for setor in buf.chunks_exact_mut(512) {
        esperar_dados()?;
        for par in setor.chunks_exact_mut(2) {
            par.copy_from_slice(&unsafe { inw(DADOS) }.to_le_bytes());
        }
    }
    Ok(())
}

pub fn gravar(lba: u32, dados: &[u8]) -> Result<(), i64> {
    preparar(lba, dados.len() / 512, GRAVAR_SETORES);
    for setor in dados.chunks_exact(512) {
        esperar_dados()?;
        for par in setor.chunks_exact(2) {
            unsafe { outw(DADOS, u16::from_le_bytes([par[0], par[1]])) };
        }
    }
    esperar_livre().map(|_| ())
}

/// Garante que o que está no cache do disco foi parar no disco de verdade.
pub fn sincronizar() -> Result<(), i64> {
    unsafe { outb(COMANDO, ESVAZIAR_CACHE) };
    esperar_livre().map(|_| ())
}
