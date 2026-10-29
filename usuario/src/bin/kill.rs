//! kill — manda um sinal para processos. Padrão: SIGTERM (15).
//! Exemplos: `kill 7`, `kill -9 7`, `kill -STOP 7`, `kill -CONT 7`.
//! Um pid negativo é um grupo inteiro: `kill -- -7` (ou `kill -TERM -7`).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn sinal(nome: &str) -> Option<i32> {
    match nome {
        "INT" => Some(2),
        "KILL" => Some(9),
        "USR1" => Some(10),
        "USR2" => Some(12),
        "ALRM" => Some(14),
        "TERM" => Some(15),
        "CONT" => Some(18),
        "STOP" => Some(19),
        "TSTP" => Some(20),
        n => n.parse().ok(),
    }
}

fn principal(args: &[&str]) -> i32 {
    let mut numero = sys::SIGTERM;
    let mut pids = &args[1..];
    if pids.first() == Some(&"--") {
        pids = &pids[1..];
    } else if let Some(opcao) = pids.first().filter(|a| a.parse::<i32>().is_err()).and_then(|a| a.strip_prefix('-')) {
        match sinal(opcao) {
            Some(s) => numero = s,
            None => {
                eprintln!("kill: sinal desconhecido: {}", opcao);
                return 2;
            }
        }
        pids = &pids[1..];
        if pids.first() == Some(&"--") {
            pids = &pids[1..];
        }
    }
    if pids.is_empty() {
        eprintln!("uso: kill [-SINAL] <pid>...");
        return 2;
    }
    let mut status = 0;
    for &texto in pids {
        let resultado = texto.parse().map_err(|_| sys::Erro(22)).and_then(|pid| sys::matar(pid, numero));
        if let Err(e) = resultado {
            eprintln!("kill: {}: {}", texto, e);
            status = 1;
        }
    }
    status
}
