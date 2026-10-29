//! init — o processo 1, o primeiro programa que o kernel executa.
//!
//! Ele abre um login em cada console (tty1 a tty4, trocados com Alt+F1..F4) e
//! o reinicia quando a sessão termina. Cada login roda numa sessão própria,
//! com o seu console como terminal de controle. O init também é o "pai
//! adotivo" dos órfãos: recolhe todo processo cujo pai morreu antes dele,
//! para que não fiquem zumbis para sempre.
//!
//! Antes dos logins, ele roda o script `/etc/rc`, que inicia os serviços
//! (o `syslogd` e o `crond`).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

const CONSOLES: usize = 4;

/// Cria a sessão de um console: um processo novo, líder de sessão, com o
/// console nos descritores 0, 1 e 2, que executa o `login`.
fn abrir_sessao(console: usize) -> Option<i32> {
    let caminho = format!("/dev/tty{}", console + 1);
    match sys::fork() {
        Ok(0) => {
            let _ = sys::nova_sessao();
            for fd in 0..3 {
                let _ = sys::fechar(fd);
            }
            // O primeiro open de um terminal pelo líder da sessão o torna o
            // terminal de controle dela.
            for _ in 0..3 {
                let _ = sys::abrir(&caminho, sys::O_RDWR);
            }
            sys::ignorar_sinal(sys::SIGINT, false);
            let erro = sys::executar("/bin/login", &["login"]);
            eprintln!("init: não consegui executar /bin/login: {}", erro);
            sys::sair(1);
        }
        Ok(pid) => Some(pid),
        Err(erro) => {
            eprintln!("init: fork falhou: {}", erro);
            None
        }
    }
}

fn principal(_args: &[&str]) -> i32 {
    // As mensagens do próprio init vão para o tty1 (sem tomá-lo como terminal).
    for _ in 0..3 {
        let _ = sys::abrir("/dev/tty1", sys::O_RDWR | sys::O_NOCTTY);
    }
    sys::ignorar_sinal(sys::SIGINT, true);

    // Os serviços do boot. Cada um vira daemon e o script termina logo.
    if sys::stat("/etc/rc").is_ok() {
        match sys::fork() {
            Ok(0) => {
                let erro = sys::executar("/bin/sh", &["sh", "/etc/rc"]);
                eprintln!("init: não consegui executar /etc/rc: {}", erro);
                sys::sair(1);
            }
            Ok(rc) => while sys::esperar(-1).is_ok_and(|(pid, _)| pid != rc) {},
            Err(_) => {}
        }
    }

    let mut sessoes = [0i32; CONSOLES];
    for (console, sessao) in sessoes.iter_mut().enumerate() {
        *sessao = abrir_sessao(console).unwrap_or(0);
    }
    loop {
        match sys::esperar(-1) {
            Ok((pid, _)) => {
                // Uma sessão terminou: abre outra no mesmo console.
                if let Some(console) = sessoes.iter().position(|&s| s == pid) {
                    if let Ok(fd) = sys::abrir(&format!("/dev/tty{}", console + 1), sys::O_WRONLY | sys::O_NOCTTY) {
                        let _ = sys::escrever_tudo(fd, "\nSessão encerrada.\n".as_bytes());
                        let _ = sys::fechar(fd);
                    }
                    let _ = sys::dormir_ms(200);
                    sessoes[console] = abrir_sessao(console).unwrap_or(0);
                }
                // senão, era um órfão adotado
            }
            Err(_) => {
                let _ = sys::dormir_ms(1000);
            }
        }
    }
}
