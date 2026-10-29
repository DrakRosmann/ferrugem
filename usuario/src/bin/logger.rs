//! logger — manda uma mensagem para o registro do sistema (o syslogd).
//!
//! ```text
//!   logger o backup terminou
//!   logger -t backup -p err falhou: disco cheio
//!   tail /var/log/mensagens
//! ```

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios::nome_do_usuario;
use ulib::*;

programa!(principal);

fn gravidade(nome: &str) -> Option<u8> {
    Some(match nome {
        "emerg" => 0,
        "alert" => 1,
        "crit" => 2,
        "err" | "error" => sys::LOG_ERR,
        "warning" | "warn" => sys::LOG_WARNING,
        "notice" => sys::LOG_NOTICE,
        "info" => sys::LOG_INFO,
        "debug" => 7,
        _ => return None,
    })
}

fn principal(args: &[&str]) -> i32 {
    let mut nome = nome_do_usuario(sys::euid());
    let mut prioridade = sys::LOG_USER | sys::LOG_NOTICE;
    let mut i = 1;
    while i + 1 < args.len() && args[i].starts_with('-') {
        match args[i] {
            "-t" => nome = String::from(args[i + 1]),
            "-p" => {
                // "err" ou "user.err": a facilidade, se vier, é ignorada (fica "user")
                let g = args[i + 1].rsplit('.').next().unwrap_or("");
                match gravidade(g) {
                    Some(g) => prioridade = sys::LOG_USER | g,
                    None => {
                        eprintln!("logger: prioridade desconhecida: {}", args[i + 1]);
                        return 1;
                    }
                }
            }
            outro => {
                eprintln!("logger: opção desconhecida: {}", outro);
                return 1;
            }
        }
        i += 2;
    }
    if i >= args.len() {
        eprintln!("uso: logger [-t nome] [-p prioridade] mensagem...");
        return 1;
    }
    sys::registrar(prioridade, &nome, &args[i..].join(" "));
    0
}
