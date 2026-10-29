//! init — o processo 1, o primeiro programa que o kernel executa.
//!
//! Ele abre o terminal nos descritores 0, 1 e 2 (entrada, saída e erro, que
//! todos os outros processos herdam), inicia o `login` e o reinicia quando a
//! sessão termina. Também é o "pai adotivo" dos órfãos: recolhe todo processo
//! cujo pai morreu antes dele, para que não fiquem zumbis para sempre.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    for _ in 0..3 {
        let _ = sys::abrir("/dev/tty", sys::O_RDWR);
    }
    sys::ignorar_sinal(sys::SIGINT, true);

    loop {
        let sessao = match sys::fork() {
            Ok(0) => {
                let erro = sys::executar("/bin/login", &["login"]);
                eprintln!("init: não consegui executar /bin/login: {}", erro);
                sys::sair(1);
            }
            Ok(pid) => pid,
            Err(erro) => {
                eprintln!("init: fork falhou: {}", erro);
                let _ = sys::dormir_ms(1000);
                continue;
            }
        };

        loop {
            match sys::esperar(-1) {
                Ok((pid, _)) if pid == sessao => break,
                Ok(_) => {} // um órfão adotado terminou
                Err(_) => break,
            }
        }
        println!("\nSessão encerrada.");
        let _ = sys::dormir_ms(200);
    }
}
