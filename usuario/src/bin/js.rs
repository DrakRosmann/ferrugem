//! js — roda JavaScript fora do navegador: `js arquivo.js`, `js -e 'código'`,
//! ou sem argumentos um REPL (uma linha de cada vez).

#![no_std]
#![no_main]

use ulib::js::{self, Valor, Vm};
use ulib::sys;
use ulib::*;

programa!(principal);

fn rodar(vm: &mut Vm, fonte: &str, mostrar: bool) -> bool {
    let r = vm.avaliar(fonte);
    let ok = r.is_ok();
    match r {
        Ok(v) => {
            if mostrar && v != Valor::Undef {
                let t = js::nativos::formatar_console(vm, &[v]);
                println!("{}", t);
            }
        }
        Err(e) => js::relatar_erro(vm, e),
    }
    js::rodar_microtarefas(vm);
    ok
}

fn principal(args: &[&str]) -> i32 {
    let mut vm = Vm::novo();
    vm.ganchos.agora_ms = sys::tempo_ms;
    vm.ganchos.console = |_, nivel, t| {
        if nivel >= 2 {
            eprintln!("{}", t);
        } else {
            println!("{}", t);
        }
    };
    match args.get(1).copied() {
        Some("-e") => {
            if rodar(&mut vm, args.get(2).copied().unwrap_or(""), true) { 0 } else { 1 }
        }
        Some(arquivo) => {
            let dados = match sys::ler_arquivo(arquivo) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("js: {}: {}", arquivo, e);
                    return 1;
                }
            };
            let t = String::from_utf8_lossy(&dados);
            if rodar(&mut vm, &t, false) { 0 } else { 1 }
        }
        None => {
            let mut leitor = Leitor::new(0);
            loop {
                print!("> ");
                let Ok(Some(linha)) = leitor.ler_linha() else { return 0 };
                rodar(&mut vm, &linha, true);
            }
        }
    }
}
