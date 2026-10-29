//! Dispositivos de bloco: os discos e suas partições (`/dev/hda`, `/dev/hda1`...).
//!
//! Um disco pode ser dividido em partições pela tabela MBR, que fica no
//! primeiro setor: a partir do byte 446, quatro entradas de 16 bytes (tipo,
//! setor inicial, quantidade de setores), e a assinatura 0x55 0xAA no fim.
//! Cada partição vira um dispositivo próprio, que enxerga só o seu pedaço do
//! disco — é sobre ele que um sistema de arquivos é montado.

use crate::ata::{UNIDADES, Unidade};
use crate::erro::*;
use crate::spin::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub struct Bloco {
    pub nome: String,
    unidade: Unidade,
    /// Primeiro setor (no disco inteiro) e quantos setores tem.
    inicio: u64,
    pub setores: u64,
}

static BLOCOS: Mutex<Vec<Arc<Bloco>>> = Mutex::new(Vec::new());

impl Bloco {
    fn conferir(&self, setor: u64, quantidade: usize) -> Result<(), i64> {
        if setor + quantidade as u64 > self.setores { Err(EIO) } else { Ok(()) }
    }

    /// Lê setores inteiros (`buf.len()` múltiplo de 512).
    pub fn ler(&self, setor: u64, buf: &mut [u8]) -> Result<(), i64> {
        self.conferir(setor, buf.len() / 512)?;
        for (i, pedaco) in buf.chunks_mut(128 * 512).enumerate() {
            self.unidade.ler(self.inicio + setor + i as u64 * 128, pedaco)?;
        }
        Ok(())
    }

    pub fn gravar(&self, setor: u64, dados: &[u8]) -> Result<(), i64> {
        self.conferir(setor, dados.len() / 512)?;
        for (i, pedaco) in dados.chunks(128 * 512).enumerate() {
            self.unidade.gravar(self.inicio + setor + i as u64 * 128, pedaco)?;
        }
        Ok(())
    }

    pub fn sincronizar(&self) -> Result<(), i64> {
        self.unidade.sincronizar()
    }

    pub fn tamanho(&self) -> u64 {
        self.setores * 512
    }

    /// Para ler `/dev/hda` como um arquivo: qualquer posição e tamanho.
    pub fn ler_bytes(&self, posicao: u64, buf: &mut [u8]) -> Result<usize, i64> {
        let n = (buf.len() as u64).min(self.tamanho().saturating_sub(posicao)) as usize;
        if n == 0 {
            return Ok(0);
        }
        let primeiro = posicao / 512;
        let ultimo = (posicao + n as u64 - 1) / 512;
        let mut setores = vec![0u8; ((ultimo - primeiro + 1) * 512) as usize];
        self.ler(primeiro, &mut setores)?;
        let desloc = (posicao % 512) as usize;
        buf[..n].copy_from_slice(&setores[desloc..desloc + n]);
        Ok(n)
    }

    /// Escreve em qualquer posição (lê, muda e regrava os setores das pontas).
    pub fn gravar_bytes(&self, posicao: u64, dados: &[u8]) -> Result<usize, i64> {
        let n = (dados.len() as u64).min(self.tamanho().saturating_sub(posicao)) as usize;
        if n == 0 {
            return Err(ENOSPC);
        }
        let primeiro = posicao / 512;
        let ultimo = (posicao + n as u64 - 1) / 512;
        let mut setores = vec![0u8; ((ultimo - primeiro + 1) * 512) as usize];
        self.ler(primeiro, &mut setores)?;
        let desloc = (posicao % 512) as usize;
        setores[desloc..desloc + n].copy_from_slice(&dados[..n]);
        self.gravar(primeiro, &setores)?;
        Ok(n)
    }
}

/// Procura os discos e lê a tabela de partições de cada um.
pub fn init() -> Vec<String> {
    let mut descricoes = Vec::new();
    for (i, unidade) in UNIDADES.iter().enumerate() {
        let Some(setores) = unidade.identificar() else { continue };
        let nome = format!("hd{}", (b'a' + i as u8) as char);
        let disco = Arc::new(Bloco { nome: nome.clone(), unidade: *unidade, inicio: 0, setores });
        let mut descricao = format!("{} ({} MiB)", nome, setores / 2048);
        let mut mbr = vec![0u8; 512];
        let mut particoes = Vec::new();
        if disco.ler(0, &mut mbr).is_ok() && mbr[510] == 0x55 && mbr[511] == 0xAA {
            for p in 0..4 {
                let e = &mbr[446 + p * 16..446 + (p + 1) * 16];
                let tipo = e[4];
                let inicio = u32::from_le_bytes(e[8..12].try_into().unwrap()) as u64;
                let tamanho = u32::from_le_bytes(e[12..16].try_into().unwrap()) as u64;
                // 0xEE é a "MBR de proteção" de um disco GPT (que ainda não lemos).
                if tipo == 0 || tipo == 0xEE || tamanho == 0 || inicio + tamanho > setores {
                    continue;
                }
                let nome_p = format!("{}{}", nome, p + 1);
                descricao.push_str(&format!(" {}", nome_p));
                particoes.push(Arc::new(Bloco { nome: nome_p, unidade: *unidade, inicio, setores: tamanho }));
            }
        }
        let mut b = BLOCOS.lock();
        b.push(disco);
        b.extend(particoes);
        descricoes.push(descricao);
    }
    descricoes
}

pub fn buscar(nome: &str) -> Option<Arc<Bloco>> {
    BLOCOS.lock().iter().find(|b| b.nome == nome).cloned()
}

pub fn listar() -> Vec<Arc<Bloco>> {
    BLOCOS.lock().clone()
}
