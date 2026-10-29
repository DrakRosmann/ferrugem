//! Discos ATA (IDE) em modo PIO: o jeito mais simples de ler e gravar um disco.
//!
//! Um PC tem dois barramentos IDE (primário em 0x1F0, secundário em 0x170),
//! cada um com até dois discos (mestre e escravo): no Linux antigo, `hda`,
//! `hdb`, `hdc` e `hdd`. Para ler um setor (512 bytes): escolhemos o disco e o
//! endereço (LBA), mandamos o comando, esperamos o bit DRQ ("dados prontos")
//! e lemos 256 palavras de 16 bits da porta de dados. Gravar é o mesmo
//! caminho ao contrário.
//!
//! "PIO" = a própria CPU copia cada palavra. É lento comparado a DMA, mas
//! não precisa de nada além de `in`/`out`. Usamos *polling* (consultamos o
//! status em laço) em vez das IRQs 14 e 15: mais simples, e o kernel não é
//! preemptivo mesmo.

use crate::erro::EIO;
use crate::porta::{inb, inw, outb, outw};

const OCUPADO: u8 = 0x80;
const FALHA: u8 = 0x20;
const DADOS_PRONTOS: u8 = 0x08;
const ERRO: u8 = 0x01;

const LER_SETORES: u8 = 0x20;
const GRAVAR_SETORES: u8 = 0x30;
const ESVAZIAR_CACHE: u8 = 0xE7;
const IDENTIFICAR: u8 = 0xEC;

/// Uma das quatro posições de disco IDE.
#[derive(Clone, Copy)]
pub struct Unidade {
    base: u16,
    controle: u16,
    escravo: bool,
}

/// hda, hdb, hdc, hdd
pub const UNIDADES: [Unidade; 4] = [
    Unidade { base: 0x1F0, controle: 0x3F6, escravo: false },
    Unidade { base: 0x1F0, controle: 0x3F6, escravo: true },
    Unidade { base: 0x170, controle: 0x376, escravo: false },
    Unidade { base: 0x170, controle: 0x376, escravo: true },
];

impl Unidade {
    fn dados(&self) -> u16 {
        self.base
    }
    fn comando(&self) -> u16 {
        self.base + 7 // escrevendo: comando; lendo: status
    }

    /// Ler o status 4 vezes dá os ~400 ns que o disco pede depois de trocar de unidade.
    fn pausa(&self) {
        for _ in 0..4 {
            unsafe { inb(self.comando()) };
        }
    }

    fn esperar_livre(&self) -> Result<u8, i64> {
        for _ in 0..10_000_000 {
            let status = unsafe { inb(self.comando()) };
            if status & OCUPADO == 0 {
                return Ok(status);
            }
        }
        Err(EIO)
    }

    fn esperar_dados(&self) -> Result<(), i64> {
        loop {
            let status = self.esperar_livre()?;
            if status & (ERRO | FALHA) != 0 {
                return Err(EIO);
            }
            if status & DADOS_PRONTOS != 0 {
                return Ok(());
            }
        }
    }

    /// Há um disco ATA aqui? Devolve quantos setores ele tem.
    pub fn identificar(&self) -> Option<u64> {
        unsafe {
            outb(self.controle, 0x02); // nIEN: o disco não gera interrupções
            outb(self.base + 6, 0xA0 | (self.escravo as u8) << 4);
            self.pausa();
            for registro in 2..=5 {
                outb(self.base + registro, 0);
            }
            outb(self.comando(), IDENTIFICAR);
            let status = inb(self.comando());
            if status == 0 || status == 0xFF {
                return None; // ninguém respondeu (ou barramento sem nada ligado)
            }
            self.esperar_livre().ok()?;
            if inb(self.base + 4) != 0 || inb(self.base + 5) != 0 {
                return None; // é um CD (ATAPI), não um disco ATA
            }
            self.esperar_dados().ok()?;
            let mut identidade = [0u16; 256];
            for palavra in identidade.iter_mut() {
                *palavra = inw(self.dados());
            }
            Some(identidade[60] as u64 | (identidade[61] as u64) << 16)
        }
    }

    fn preparar(&self, lba: u64, setores: usize, comando: u8) {
        unsafe {
            outb(self.base + 6, 0xE0 | (self.escravo as u8) << 4 | ((lba >> 24) & 0x0F) as u8); // LBA
            outb(self.base + 2, setores as u8);
            outb(self.base + 3, lba as u8);
            outb(self.base + 4, (lba >> 8) as u8);
            outb(self.base + 5, (lba >> 16) as u8);
            outb(self.comando(), comando);
        }
    }

    /// Lê `buf.len() / 512` setores (no máximo 255) a partir de `lba`.
    pub fn ler(&self, lba: u64, buf: &mut [u8]) -> Result<(), i64> {
        self.preparar(lba, buf.len() / 512, LER_SETORES);
        for setor in buf.chunks_exact_mut(512) {
            self.esperar_dados()?;
            for par in setor.chunks_exact_mut(2) {
                par.copy_from_slice(&unsafe { inw(self.dados()) }.to_le_bytes());
            }
        }
        Ok(())
    }

    pub fn gravar(&self, lba: u64, dados: &[u8]) -> Result<(), i64> {
        self.preparar(lba, dados.len() / 512, GRAVAR_SETORES);
        for setor in dados.chunks_exact(512) {
            self.esperar_dados()?;
            for par in setor.chunks_exact(2) {
                unsafe { outw(self.dados(), u16::from_le_bytes([par[0], par[1]])) };
            }
        }
        self.esperar_livre().map(|_| ())
    }

    /// Garante que o que está no cache do disco foi parar no disco de verdade.
    pub fn sincronizar(&self) -> Result<(), i64> {
        unsafe {
            outb(self.base + 6, 0xA0 | (self.escravo as u8) << 4);
            outb(self.comando(), ESVAZIAR_CACHE);
        }
        self.esperar_livre().map(|_| ())
    }
}
