//! ping — manda pedidos de eco (ICMP) e mede quanto a resposta demora.
//!
//! `ping [-c N] host`: N pedidos (4, se não disser), um por segundo.

#![no_std]
#![no_main]

use ulib::rede;
use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let (mut quantos, mut host) = (4u16, None);
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "-c" => {
                quantos = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(4);
                i += 1;
            }
            h => host = Some(h),
        }
        i += 1;
    }
    let Some(host) = host else {
        eprintln!("uso: ping [-c N] host");
        return 2;
    };
    let ip = match rede::resolver(host) {
        Ok(ip) => ip,
        Err(e) => {
            eprintln!("ping: {}", e);
            return 1;
        }
    };
    let fd = match sys::socket_inet(sys::SOCK_DGRAM, sys::IPPROTO_ICMP) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("ping: socket: {}", e);
            return 1;
        }
    };
    println!("PING {} ({}): 56 bytes de dados", host, rede::texto_ip(ip));
    let (mut recebidos, mut tempos) = (0u32, Vec::new());
    for seq in 1..=quantos {
        // tipo 8 (pedido de eco), código 0, soma e id (o kernel preenche), número
        let mut m = vec![8u8, 0, 0, 0, 0, 0];
        m.extend_from_slice(&seq.to_be_bytes());
        m.extend((0..56).map(|i| i as u8));
        let inicio = sys::tempo_ms();
        if let Err(e) = sys::enviar_para_inet(fd, &m, ip, 0) {
            eprintln!("ping: {}", e);
            return 1;
        }
        let prazo = inicio + 1000;
        let mut buf = [0u8; 1500];
        loop {
            let agora = sys::tempo_ms();
            if agora >= prazo {
                println!("seq={}: sem resposta", seq);
                break;
            }
            if sys::poll(&[(fd, sys::POLLIN)], (prazo - agora) as i32).map_or(true, |r| r[0] & sys::POLLIN == 0) {
                continue;
            }
            let Ok((n, de, _)) = sys::receber_de_inet(fd, &mut buf) else { continue };
            if n >= 8 && buf[0] == 0 && buf[6..8] == seq.to_be_bytes() {
                let tempo = sys::tempo_ms() - inicio;
                println!("{} bytes de {}: seq={} tempo={} ms", n, rede::texto_ip(de), seq, tempo);
                recebidos += 1;
                tempos.push(tempo);
                break;
            }
        }
        if seq < quantos {
            let falta = (inicio + 1000).saturating_sub(sys::tempo_ms());
            let _ = sys::dormir_ms(falta);
        }
    }
    let perdidos = (quantos as u32 - recebidos) * 100 / quantos.max(1) as u32;
    println!("--- {} ---", host);
    println!("{} enviados, {} recebidos, {}% perdidos", quantos, recebidos, perdidos);
    if let (Some(min), Some(max)) = (tempos.iter().min(), tempos.iter().max()) {
        let media = tempos.iter().sum::<u64>() / tempos.len() as u64;
        println!("tempo mín/média/máx = {}/{}/{} ms", min, media, max);
    }
    if recebidos > 0 { 0 } else { 1 }
}
