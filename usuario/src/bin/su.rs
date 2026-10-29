//! su — abre um shell como outro usuário (o root, se nenhum for dito).
//!
//! É um programa *setuid root*: roda com euid 0 mesmo quando quem chama é
//! um usuário comum. Por isso consegue ler o /etc/shadow para conferir a
//! senha — e por isso precisa ser muito cuidadoso com o que faz.

#![no_std]
#![no_main]

use ulib::io;
use ulib::sys;
use ulib::usuarios;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let nome = args.get(1).copied().unwrap_or("root");
    let Some(alvo) = usuarios::por_nome(nome) else {
        eprintln!("su: usuário {} não existe", nome);
        return 1;
    };
    if sys::euid() != 0 {
        eprintln!("su: precisa ser setuid root (chmod 4755 /bin/su)");
        return 1;
    }
    // O root não precisa de senha para virar ninguém.
    if sys::uid() != 0 {
        let senha = io::ler_senha("Senha: ").unwrap_or_default();
        if !usuarios::conferir_senha(nome, &senha) {
            let _ = sys::dormir_ms(1000);
            eprintln!("su: senha incorreta");
            return 1;
        }
    }
    if sys::mudar_gid(alvo.gid).is_err() || sys::mudar_uid(alvo.uid).is_err() {
        eprintln!("su: não consegui trocar de usuário");
        return 1;
    }
    let ambiente = [
        format!("HOME={}", alvo.casa),
        format!("USER={}", alvo.nome),
        format!("SHELL={}", alvo.shell),
        String::from("PATH=/bin"),
        String::from("TERM=linux"),
    ];
    let shell = alvo.shell.rsplit('/').next().unwrap_or("sh");
    let erro = sys::executar_com_ambiente(&alvo.shell, &[shell], &ambiente);
    eprintln!("su: {}: {}", alvo.shell, erro);
    1
}
