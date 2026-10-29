//! Carregador de executáveis ELF (o formato de programas do Linux e da maioria dos Unix).
//!
//! Um ELF tem um cabeçalho e uma lista de "program headers". Os do tipo
//! `PT_LOAD` dizem: "copie N bytes do arquivo, a partir do deslocamento X,
//! para o endereço virtual V, e zere o resto até M bytes" (o resto é o .bss).
//!
//! Dois tipos de ELF executam:
//! - `ET_EXEC`: os endereços são absolutos (nossos programas, em 0x40000000).
//! - `ET_DYN`: os endereços são relativos a uma *base* que escolhemos (um
//!   executável PIE, ou uma biblioteca). Sorteamos a base (ASLR).
//!
//! Um programa ligado a bibliotecas compartilhadas tem um `PT_INTERP`: o
//! caminho do *carregador dinâmico* (ld.so). O kernel carrega os dois e
//! começa pelo carregador, que acha as bibliotecas, resolve os símbolos e só
//! então pula para o programa (ele sabe onde, pelo vetor auxiliar).

use crate::erro::*;
use crate::paginacao::{
    ESCRITA, EspacoEnderecos, Fonte, NAO_EXECUTAR, PROT_ESCREVER, PROT_EXECUTAR, PROT_LER, Regiao, USUARIO,
    USUARIO_INICIO,
};

/// O programa (código, dados e .bss) precisa caber abaixo de 256 GiB; acima
/// ficam o mmap e a pilha.
pub const LIMITE_IMAGEM: u64 = 0x40_0000_0000;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const ET_EXEC: u64 = 2;
const ET_DYN: u64 = 3;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

/// O que o carregador descobre sobre o programa.
pub struct Programa {
    pub entrada: u64,
    /// O carregador dinâmico que ele pede (PT_INTERP), se pedir.
    pub interpretador: Option<alloc::string::String>,
    /// Onde a imagem termina (o heap começa logo depois).
    pub fim: u64,
    /// Endereço dos program headers na memória (a libc procura o TLS neles), ou 0.
    pub cabecalhos: u64,
    pub quantidade_cabecalhos: u64,
}

fn ler<const N: usize>(dados: &[u8], deslocamento: usize) -> Result<[u8; N], i64> {
    dados
        .get(deslocamento..deslocamento + N)
        .and_then(|b| b.try_into().ok())
        .ok_or(ENOEXEC)
}

fn u16_em(d: &[u8], o: usize) -> Result<u64, i64> {
    Ok(u16::from_le_bytes(ler(d, o)?) as u64)
}
fn u32_em(d: &[u8], o: usize) -> Result<u32, i64> {
    Ok(u32::from_le_bytes(ler(d, o)?))
}
fn u64_em(d: &[u8], o: usize) -> Result<u64, i64> {
    Ok(u64::from_le_bytes(ler(d, o)?))
}

/// "\x7fELF", 64 bits, little-endian, x86_64 (0x3E); devolve o tipo (ET_EXEC ou ET_DYN).
fn tipo(dados: &[u8]) -> Result<u64, i64> {
    if dados.get(0..6) != Some(b"\x7fELF\x02\x01") || u16_em(dados, 18)? != 0x3E {
        return Err(ENOEXEC);
    }
    match u16_em(dados, 16)? {
        t @ (ET_EXEC | ET_DYN) => Ok(t),
        _ => Err(ENOEXEC),
    }
}

/// É um ELF relocável (PIE ou biblioteca), que pode ir para qualquer base?
pub fn relocavel(dados: &[u8]) -> Result<bool, i64> {
    Ok(tipo(dados)? == ET_DYN)
}

/// Os endereços (antes da base) que os PT_LOAD ocupam: [menor, maior).
pub fn extensao(dados: &[u8]) -> Result<(u64, u64), i64> {
    let tabela = u64_em(dados, 32)? as usize;
    let tamanho_entrada = u16_em(dados, 54)? as usize;
    let (mut menor, mut maior) = (u64::MAX, 0);
    for i in 0..u16_em(dados, 56)? as usize {
        let o = tabela + i * tamanho_entrada;
        if u32_em(dados, o)? == PT_LOAD {
            let virt = u64_em(dados, o + 16)?;
            menor = menor.min(virt);
            maior = maior.max(virt.checked_add(u64_em(dados, o + 40)?).ok_or(ENOEXEC)?);
        }
    }
    if menor >= maior { Err(ENOEXEC) } else { Ok((menor & !0xFFF, maior)) }
}

/// Carrega os segmentos no espaço do processo. `base`: somada a todos os
/// endereços (só para ET_DYN; ET_EXEC exige 0). Tudo precisa caber abaixo de `limite`.
pub fn carregar(dados: &[u8], espaco: &mut EspacoEnderecos, base: u64, limite: u64) -> Result<Programa, i64> {
    if tipo(dados)? == ET_EXEC && base != 0 {
        return Err(ENOEXEC);
    }
    let entrada = base + u64_em(dados, 24)?;
    let tabela = u64_em(dados, 32)? as usize;
    let tamanho_entrada = u16_em(dados, 54)? as usize;
    let quantidade = u16_em(dados, 56)? as usize;
    let mut interpretador = None;

    let mut fim_imagem = USUARIO_INICIO;
    let mut cabecalhos = 0;
    for i in 0..quantidade {
        let o = tabela + i * tamanho_entrada;
        let tipo = u32_em(dados, o)?;
        if tipo == PT_PHDR {
            cabecalhos = base + u64_em(dados, o + 16)?;
        }
        if tipo == PT_INTERP {
            let inicio = u64_em(dados, o + 8)? as usize;
            let tamanho = u64_em(dados, o + 32)? as usize;
            let caminho = dados.get(inicio..inicio + tamanho).ok_or(ENOEXEC)?;
            let caminho = caminho.split(|&b| b == 0).next().unwrap_or(&[]);
            interpretador = Some(core::str::from_utf8(caminho).map_err(|_| ENOEXEC)?.into());
        }
        if tipo != PT_LOAD {
            continue;
        }
        let flags = u32_em(dados, o + 4)?;
        let deslocamento = u64_em(dados, o + 8)? as usize;
        let virt = base + u64_em(dados, o + 16)?;
        let no_arquivo = u64_em(dados, o + 32)? as usize;
        let na_memoria = u64_em(dados, o + 40)?;
        if na_memoria == 0 {
            continue;
        }
        let fim = virt.checked_add(na_memoria).ok_or(ENOEXEC)?;
        if virt < USUARIO_INICIO || fim > limite || no_arquivo as u64 > na_memoria {
            return Err(ENOEXEC);
        }
        let conteudo = dados.get(deslocamento..deslocamento + no_arquivo).ok_or(ENOEXEC)?;

        // Só o código é executável (NX no resto); só os dados são graváveis.
        let mut permissao = USUARIO;
        let mut prot = PROT_LER;
        if flags & PF_W != 0 {
            permissao |= ESCRITA;
            prot |= PROT_ESCREVER;
        }
        if flags & PF_X != 0 {
            prot |= PROT_EXECUTAR;
        } else {
            permissao |= NAO_EXECUTAR;
        }
        if !espaco.alocar_regiao(virt, fim, permissao) {
            return Err(ENOMEM);
        }
        espaco.registrar_regiao(Regiao {
            inicio: virt & !0xFFF,
            fim: (fim + 0xFFF) & !0xFFF,
            prot,
            compartilhada: false,
            fonte: Fonte::Anonima, // as páginas já estão carregadas
        });
        // As páginas novas já vêm zeradas: o .bss não precisa de mais nada.
        if !espaco.gravar(virt, conteudo) {
            return Err(EFAULT);
        }
        fim_imagem = fim_imagem.max(fim);
        // Sem PT_PHDR: se a tabela de headers estiver dentro deste segmento, achamos o endereço.
        let t = tabela as u64;
        if cabecalhos == 0 && (deslocamento as u64..(deslocamento + no_arquivo) as u64).contains(&t) {
            cabecalhos = virt + (t - deslocamento as u64);
        }
    }
    if !(USUARIO_INICIO..fim_imagem).contains(&entrada) {
        return Err(ENOEXEC);
    }
    Ok(Programa { entrada, interpretador, fim: fim_imagem, cabecalhos, quantidade_cabecalhos: quantidade as u64 })
}
