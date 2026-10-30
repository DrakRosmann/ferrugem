//! rede — a placa, o endereço, os vizinhos (ARP) e as conexões (do /proc/rede).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let texto = match sys::ler_arquivo("/proc/rede") {
        Ok(t) => String::from_utf8_lossy(&t).into_owned(),
        Err(e) => {
            eprintln!("rede: /proc/rede: {}", e);
            return 1;
        }
    };
    let mut conexoes = Vec::new();
    for linha in texto.lines() {
        let partes: Vec<&str> = linha.split_whitespace().collect();
        match partes.first().copied() {
            Some("placa") => println!("placa:     {}", partes[1..].join(" ")),
            Some("ip") if partes.len() >= 6 => {
                println!("endereço:  {}", partes[1]);
                println!("roteador:  {}", partes[3]);
                println!("DNS:       {}", partes[5]);
            }
            Some("arp") if partes.len() >= 3 => println!("vizinho:   {} ({})", partes[1], partes[2]),
            Some("tcp" | "udp" | "icmp") => conexoes.push(partes),
            _ => println!("{}", linha),
        }
    }
    if !conexoes.is_empty() {
        println!();
        println!("{:<5} {:<22} {:<22} {}", "prot", "local", "remoto", "estado");
        for c in conexoes {
            println!("{:<5} {:<22} {:<22} {}", c[0], c[1], c.get(2).unwrap_or(&""), c.get(3..).map_or(String::new(), |r| r.join(" ")));
        }
    }
    0
}
