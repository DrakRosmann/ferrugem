//! login — pede usuário e senha e abre o shell com a identidade da pessoa.
//!
//! Roda como root (o init o executa). Depois de conferir a senha no
//! /etc/shadow, troca de identidade com setgid/setuid — daí em diante não há
//! volta: o processo é daquele usuário — e executa o shell dele.

#![no_std]
#![no_main]

use ulib::io;
use ulib::sys;
use ulib::usuarios::{self, Usuario};
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    sys::ignorar_sinal(sys::SIGINT, true);
    let mut entrada = Leitor::new(io::ENTRADA);
    loop {
        if let Ok(aviso) = sys::ler_arquivo("/etc/issue") {
            let _ = sys::escrever_tudo(io::SAIDA, &aviso);
        }
        print!("{} login: ", sys::uname().maquina);
        let nome = match entrada.ler_linha() {
            Ok(Some(nome)) => String::from(nome.trim()),
            Ok(None) => {
                entrada.continuar(); // Ctrl+D: pergunta de novo
                println!();
                continue;
            }
            Err(_) => continue,
        };
        if nome.is_empty() {
            continue;
        }
        let senha = io::ler_senha("Senha: ").unwrap_or_default();
        match usuarios::por_nome(&nome) {
            Some(u) if usuarios::conferir_senha(&nome, &senha) => iniciar_sessao(u),
            _ => {
                let _ = sys::dormir_ms(1000); // atrasa quem tenta adivinhar
                println!("Login incorreto");
            }
        }
    }
}

fn iniciar_sessao(u: Usuario) -> ! {
    // Primeiro o grupo: depois do setuid não teríamos mais permissão para isso.
    if sys::mudar_gid(u.gid).is_err() || sys::mudar_uid(u.uid).is_err() {
        eprintln!("login: não consegui trocar de usuário");
        sys::sair(1);
    }
    // Uma sessão nova, dona do terminal: daqui saem os jobs do shell.
    let _ = sys::nova_sessao();
    let _ = sys::dar_terminal(io::ENTRADA, sys::pid());
    if sys::mudar_diretorio(&u.casa).is_err() {
        eprintln!("login: diretório {} não existe; usando /", u.casa);
        let _ = sys::mudar_diretorio("/");
    }
    if let Ok(motd) = sys::ler_arquivo("/etc/motd") {
        let _ = sys::escrever_tudo(io::SAIDA, &motd);
    }
    let ambiente = [
        format!("HOME={}", u.casa),
        format!("USER={}", u.nome),
        format!("LOGNAME={}", u.nome),
        format!("SHELL={}", u.shell),
        String::from("PATH=/bin"),
        String::from("TERM=linux"),
    ];
    let nome = u.shell.rsplit('/').next().unwrap_or("sh");
    let erro = sys::executar_com_ambiente(&u.shell, &[nome], &ambiente);
    eprintln!("login: não consegui executar {}: {}", u.shell, erro);
    sys::sair(1)
}
