//! kill [-SINAL] pid...: envia um sinal (TERM por padrão). Um pid negativo
//! indica um grupo de processos. `kill -l` lista os sinais. (No sh, o
//! `kill` interno também aceita jobs: `kill %1`.)

#![no_std]
#![no_main]

use ferrugem_user::{entry, eprintln, print, println, rt, sys};

entry!(main);

fn main() -> i32 {
    let mut signal = sys::SIGTERM;
    let mut status = 0;
    let mut targets = 0;
    for arg in rt::args().skip(1) {
        if arg == "-l" {
            for (number, name) in sys::SIGNAL_NAMES.iter().enumerate().skip(1) {
                print!("{number:>2}) SIG{name:<8}");
                if number % 4 == 0 {
                    println!();
                }
            }
            return 0;
        }
        if targets == 0
            && let Some(name) = arg.strip_prefix('-')
            && let Some(parsed) = sys::parse_signal(name)
        {
            signal = parsed;
            continue;
        }
        targets += 1;
        let Ok(pid) = arg.parse::<i64>() else {
            eprintln!("kill: pid inválido: {arg}");
            status = 1;
            continue;
        };
        if let Err(errno) = sys::kill(pid, signal) {
            eprintln!("kill: {pid}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    if targets == 0 {
        eprintln!("uso: kill [-SINAL] pid...");
        return 2;
    }
    status
}
