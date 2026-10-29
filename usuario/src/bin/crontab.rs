//! crontab — mostra, instala ou apaga a sua tabela de tarefas agendadas.
//!
//! ```text
//!   crontab -l               mostra a tabela
//!   crontab tarefas.txt      instala a tabela a partir de um arquivo
//!   crontab -                ...ou do que vier pela entrada
//!   crontab -r               apaga a tabela
//!   crontab -u nome -l       (root) a tabela de outro usuário
//! ```
//!
//! Formato de cada linha: `minuto hora dia mês dia-da-semana comando`
//! (veja o `crond`). As tabelas ficam em /var/spool/cron/<usuário>, onde só o
//! root escreve: por isso este programa é setuid root. Ele lê o arquivo que
//! você indicou *com as suas permissões* (num processo filho que deixa de ser
//! root), para não servir de porta para ler arquivos alheios.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios;
use ulib::*;

programa!(principal);

const DIRETORIO: &str = "/var/spool/cron";

/// Lê um arquivo (ou a entrada, com "-") como o usuário de verdade.
fn ler_como_usuario(caminho: &str) -> Result<Vec<u8>, String> {
    let (leitura, escrita) = sys::pipe().map_err(|e| format!("pipe: {}", e))?;
    match sys::fork() {
        Ok(0) => {
            let _ = sys::fechar(leitura);
            if sys::mudar_gid(sys::gid()).is_err() || sys::mudar_uid(sys::uid()).is_err() {
                sys::sair(2);
            }
            let dados = if caminho == "-" {
                let mut tudo = Vec::new();
                let mut buf = [0u8; 512];
                while let Ok(n) = sys::ler(io::ENTRADA, &mut buf) {
                    if n == 0 {
                        break;
                    }
                    tudo.extend_from_slice(&buf[..n]);
                }
                Ok(tudo)
            } else {
                sys::ler_arquivo(caminho)
            };
            match dados {
                Ok(d) => {
                    let _ = sys::escrever_tudo(escrita, &d);
                    sys::sair(0);
                }
                Err(e) => {
                    eprintln!("crontab: {}: {}", caminho, e);
                    sys::sair(1);
                }
            }
        }
        Ok(filho) => {
            let _ = sys::fechar(escrita);
            let mut dados = Vec::new();
            let mut buf = [0u8; 512];
            while let Ok(n) = sys::ler(leitura, &mut buf) {
                if n == 0 {
                    break;
                }
                dados.extend_from_slice(&buf[..n]);
            }
            let _ = sys::fechar(leitura);
            match sys::esperar(filho) {
                Ok((_, status)) if sys::codigo_de_saida(status) == Some(0) => Ok(dados),
                _ => Err(String::new()),
            }
        }
        Err(e) => Err(format!("fork: {}", e)),
    }
}

/// Confere o formato por cima: 5 campos e um comando, ou @atalho e um comando.
fn conferir(texto: &str) -> Result<(), String> {
    for (i, linha) in texto.lines().enumerate() {
        let linha = linha.trim();
        if linha.is_empty() || linha.starts_with('#') {
            continue;
        }
        let palavras = linha.split_whitespace().count();
        let minimo = if linha.starts_with('@') { 2 } else { 6 };
        if palavras < minimo {
            return Err(format!("linha {}: faltam campos: {}", i + 1, linha));
        }
    }
    Ok(())
}

fn principal(args: &[&str]) -> i32 {
    let mut args: Vec<&str> = args[1..].to_vec();
    let mut nome = usuarios::nome_do_usuario(sys::uid());
    if args.first() == Some(&"-u") {
        if sys::uid() != 0 {
            eprintln!("crontab: só o root mexe na tabela de outro usuário");
            return 1;
        }
        let Some(outro) = args.get(1) else {
            eprintln!("crontab: -u precisa de um nome");
            return 1;
        };
        if usuarios::por_nome(outro).is_none() {
            eprintln!("crontab: usuário desconhecido: {}", outro);
            return 1;
        }
        nome = String::from(*outro);
        args.drain(..2);
    }
    let caminho = format!("{}/{}", DIRETORIO, nome);
    match args.first() {
        Some(&"-l") => match sys::ler_arquivo(&caminho) {
            Ok(dados) => {
                let _ = sys::escrever_tudo(io::SAIDA, &dados);
                0
            }
            Err(_) => {
                eprintln!("crontab: {} não tem tabela", nome);
                1
            }
        },
        Some(&"-r") => match sys::remover(&caminho) {
            Ok(()) => 0,
            Err(_) => {
                eprintln!("crontab: {} não tem tabela", nome);
                1
            }
        },
        Some(arquivo) if !arquivo.starts_with('-') || *arquivo == "-" => {
            let dados = match ler_como_usuario(arquivo) {
                Ok(d) => d,
                Err(e) => {
                    if !e.is_empty() {
                        eprintln!("crontab: {}", e);
                    }
                    return 1;
                }
            };
            let texto = String::from_utf8_lossy(&dados);
            if let Err(e) = conferir(&texto) {
                eprintln!("crontab: {} (nada foi instalado)", e);
                return 1;
            }
            let _ = sys::criar_diretorio("/var/spool");
            let _ = sys::criar_diretorio(DIRETORIO);
            let gravado = sys::abrir_com_permissoes(&caminho, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o600)
                .and_then(|fd| sys::escrever_tudo(fd, texto.as_bytes()).map(|_| fd))
                .map(|fd| sys::fechar(fd));
            if let Err(e) = gravado {
                eprintln!("crontab: {}: {}", caminho, e);
                return 1;
            }
            sys::registrar(sys::LOG_CRON | sys::LOG_INFO, "crontab", &format!("({}) tabela instalada", nome));
            0
        }
        _ => {
            eprintln!("uso: crontab [-u usuário] {{arquivo | - | -l | -r}}");
            1
        }
    }
}
