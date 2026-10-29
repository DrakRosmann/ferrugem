//! initrd: o "disco inicial" que o bootloader carrega na memória junto com o kernel.
//!
//! É um arquivo `tar` comum (formato ustar), montado pelo Makefile com os
//! programas de usuário. Um tar é uma sequência de blocos de 512 bytes:
//! um cabeçalho (nome, permissões, dono, tamanho em octal, tipo) seguido do conteúdo.

use crate::rtc;
use crate::vfs::{self, Conteudo, Meta};

fn texto(campo: &[u8]) -> &str {
    let fim = campo.iter().position(|&b| b == 0).unwrap_or(campo.len());
    core::str::from_utf8(&campo[..fim]).unwrap_or("")
}

fn octal(campo: &[u8]) -> usize {
    texto(campo).trim().bytes().fold(0, |n, d| n * 8 + (d.wrapping_sub(b'0') as usize & 7))
}

/// Copia o conteúdo do tar para o VFS. Devolve quantos arquivos foram criados.
pub fn carregar(dados: &[u8]) -> usize {
    let mut posicao = 0;
    let mut arquivos = 0;
    while posicao + 512 <= dados.len() {
        let cabecalho = &dados[posicao..posicao + 512];
        if cabecalho.iter().all(|&b| b == 0) {
            break; // dois blocos zerados marcam o fim
        }
        let tamanho = octal(&cabecalho[124..136]);
        let tipo = cabecalho[156];
        let nome = if &cabecalho[257..262] == b"ustar" && cabecalho[345] != 0 {
            alloc::format!("{}/{}", texto(&cabecalho[345..500]), texto(&cabecalho[0..100]))
        } else {
            alloc::string::String::from(texto(&cabecalho[0..100]))
        };
        let caminho = vfs::normalizar("/", &nome);
        let meta = Meta {
            permissoes: octal(&cabecalho[100..108]) as u32 & 0o7777,
            uid: octal(&cabecalho[108..116]) as u32,
            gid: octal(&cabecalho[116..124]) as u32,
            modificado: rtc::agora(),
        };
        let inicio = posicao + 512;
        let conteudo = dados.get(inicio..inicio + tamanho).unwrap_or(&[]);

        match tipo {
            b'5' => {
                vfs::criar_diretorios(&caminho);
                if let Ok(no) = vfs::buscar(&caminho) {
                    let _ = no.alterar(Some(meta.permissoes), Some(meta.uid), Some(meta.gid));
                }
            }
            b'0' | 0 if caminho != "/" => {
                if let Some((pai, _)) = caminho.rsplit_once('/') {
                    vfs::criar_diretorios(pai);
                }
                if vfs::criar_na_memoria(&caminho, Conteudo::Arquivo(conteudo.to_vec()), Some(meta)).is_ok() {
                    arquivos += 1;
                }
            }
            _ => {}
        }
        posicao = inicio + tamanho.div_ceil(512) * 512;
    }
    arquivos
}
