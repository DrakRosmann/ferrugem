//! ps — lista os processos.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios::nome_do_usuario;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    println!("  PID   PAI GRUPO  USUÁRIO   ESTADO      MEMÓRIA     CPU  NOME");
    for p in sys::processos() {
        let estado = match p.estado {
            0 => "pronto",
            1 => "rodando",
            2 => "dormindo",
            3 => "zumbi",
            _ => "parado",
        };
        let cpu = p.tiques * 10; // 1 tique = 10 ms
        println!(
            "{:>5} {:>5} {:>5}  {:<9} {:<9} {:>6} KiB {:>4}.{:02}s  {}",
            p.pid,
            p.pai,
            p.pgid,
            nome_do_usuario(p.uid),
            estado,
            p.memoria_kib,
            cpu / 1000,
            cpu % 1000 / 10,
            p.nome
        );
    }
    0
}
