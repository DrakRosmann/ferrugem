//! Usuários e grupos: /etc/passwd, /etc/group e /etc/shadow.
//!
//! ```text
//!   /etc/passwd   nome:x:uid:gid:descrição:diretório pessoal:shell   (todos leem)
//!   /etc/group    nome:x:gid:membros
//!   /etc/shadow   nome:sal:hash da senha                             (só o root lê)
//! ```

use crate::sha256::{hex, sha256};
use crate::sys::{self, Erro};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone)]
pub struct Usuario {
    pub nome: String,
    pub uid: u32,
    pub gid: u32,
    pub descricao: String,
    pub casa: String,
    pub shell: String,
}

fn linhas(caminho: &str) -> Vec<Vec<String>> {
    let dados = sys::ler_arquivo(caminho).unwrap_or_default();
    String::from_utf8_lossy(&dados)
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').map(String::from).collect())
        .collect()
}

pub fn todos() -> Vec<Usuario> {
    linhas("/etc/passwd")
        .into_iter()
        .filter(|c| c.len() >= 7)
        .map(|c| Usuario {
            nome: c[0].clone(),
            uid: c[2].parse().unwrap_or(u32::MAX),
            gid: c[3].parse().unwrap_or(u32::MAX),
            descricao: c[4].clone(),
            casa: c[5].clone(),
            shell: c[6].clone(),
        })
        .collect()
}

pub fn por_nome(nome: &str) -> Option<Usuario> {
    todos().into_iter().find(|u| u.nome == nome)
}

pub fn por_uid(uid: u32) -> Option<Usuario> {
    todos().into_iter().find(|u| u.uid == uid)
}

/// O nome do usuário, ou o número se ele não estiver no /etc/passwd.
pub fn nome_do_usuario(uid: u32) -> String {
    por_uid(uid).map(|u| u.nome).unwrap_or_else(|| format!("{}", uid))
}

pub fn nome_do_grupo(gid: u32) -> String {
    linhas("/etc/group")
        .into_iter()
        .find(|c| c.len() >= 3 && c[2].parse() == Ok(gid))
        .map(|c| c[0].clone())
        .unwrap_or_else(|| format!("{}", gid))
}

pub fn gid_do_grupo(nome: &str) -> Option<u32> {
    linhas("/etc/group").into_iter().find(|c| c.len() >= 3 && c[0] == nome).and_then(|c| c[2].parse().ok())
}

/// sha256 de (sal + senha), refeito mil vezes para deixar a força bruta mais lenta.
pub fn hash_da_senha(sal: &str, senha: &str) -> String {
    let mut h = sha256(format!("{}{}", sal, senha).as_bytes());
    for _ in 0..1000 {
        let mut entrada = Vec::from(&h[..]);
        entrada.extend_from_slice(senha.as_bytes());
        h = sha256(&entrada);
    }
    hex(&h)
}

/// Confere a senha (precisa conseguir ler o /etc/shadow: root ou setuid root).
pub fn conferir_senha(nome: &str, senha: &str) -> bool {
    linhas("/etc/shadow")
        .into_iter()
        .any(|c| c.len() >= 3 && c[0] == nome && hash_da_senha(&c[1], senha) == c[2])
}

/// Grava uma senha nova no /etc/shadow, com um sal novo.
pub fn trocar_senha(nome: &str, senha: &str) -> Result<(), Erro> {
    let sal = hex(&sha256(format!("{}{}{}", sys::tempo_ms(), sys::pid(), nome).as_bytes())[..8]);
    let mut texto = String::new();
    let mut achou = false;
    for campos in linhas("/etc/shadow") {
        if campos.first().map(|n| n.as_str()) == Some(nome) {
            texto.push_str(&format!("{}:{}:{}\n", nome, sal, hash_da_senha(&sal, senha)));
            achou = true;
        } else {
            texto.push_str(&campos.join(":"));
            texto.push('\n');
        }
    }
    if !achou {
        return Err(sys::ENOENT);
    }
    let fd = sys::abrir("/etc/shadow", sys::O_WRONLY | sys::O_TRUNC)?;
    let r = sys::escrever_tudo(fd, texto.as_bytes());
    let _ = sys::fechar(fd);
    r
}
