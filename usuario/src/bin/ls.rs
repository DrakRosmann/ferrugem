//! ls — lista diretórios. `-l`: formato longo (tipo e tamanho). `-a`: inclui ocultos.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios::{nome_do_grupo, nome_do_usuario};
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut longo = false;
    let mut ocultos = false;
    let mut caminhos = Vec::new();
    for &arg in &args[1..] {
        match arg.strip_prefix('-') {
            Some(opcoes) if !opcoes.is_empty() => {
                longo |= opcoes.contains('l');
                ocultos |= opcoes.contains('a');
            }
            _ => caminhos.push(arg),
        }
    }
    if caminhos.is_empty() {
        caminhos.push(".");
    }

    let mut status = 0;
    for (i, &caminho) in caminhos.iter().enumerate() {
        let info = match sys::stat(caminho) {
            Ok(info) => info,
            Err(e) => {
                eprintln!("ls: {}: {}", caminho, e);
                status = 1;
                continue;
            }
        };
        if !info.eh_diretorio() {
            mostrar(&[(String::from(caminho), info)], longo);
            continue;
        }
        if caminhos.len() > 1 {
            println!("{}{}:", if i > 0 { "\n" } else { "" }, caminho);
        }
        match sys::listar_diretorio(caminho) {
            Ok(mut entradas) => {
                entradas.sort_by(|a, b| a.nome.cmp(&b.nome));
                let itens: Vec<(String, sys::Stat)> = entradas
                    .into_iter()
                    .filter(|e| ocultos || !e.nome.starts_with('.'))
                    .filter_map(|e| {
                        let completo = format!("{}/{}", caminho, e.nome);
                        sys::stat(&completo).ok().map(|s| (e.nome, s))
                    })
                    .collect();
                mostrar(&itens, longo);
            }
            Err(e) => {
                eprintln!("ls: {}: {}", caminho, e);
                status = 1;
            }
        }
    }
    status
}

fn colorido(nome: &str, info: &sys::Stat) -> String {
    if info.eh_diretorio() {
        format!("\x1b[94m{}/\x1b[0m", nome)
    } else if info.eh_dispositivo() {
        format!("\x1b[93m{}\x1b[0m", nome)
    } else {
        String::from(nome)
    }
}

fn mostrar(itens: &[(String, sys::Stat)], longo: bool) {
    if longo {
        for (nome, info) in itens {
            let tipo = if info.eh_diretorio() { 'd' } else if info.eh_dispositivo() { 'c' } else { '-' };
            let permissoes = texto_permissoes(info.modo);
            let dono = nome_do_usuario(info.uid);
            let grupo = nome_do_grupo(info.gid);
            let tamanho = if info.eh_diretorio() { String::from("-") } else { format!("{}", info.tamanho) };
            let data = if info.modificado == 0 {
                String::from("                 ")
            } else {
                sys::formatar_data(info.modificado)
            };
            println!(
                "{}{} {:<8} {:<8} {:>7}  {}  {}",
                tipo,
                permissoes,
                dono,
                grupo,
                tamanho,
                &data[..data.len() - 3],
                colorido(nome, info)
            );
        }
        return;
    }
    // Em colunas, como o ls de verdade
    let largura = itens.iter().map(|(n, i)| n.chars().count() + i.eh_diretorio() as usize).max().unwrap_or(0) + 2;
    let por_linha = (78 / largura.max(1)).max(1);
    let mut linha = String::new();
    for (i, (nome, info)) in itens.iter().enumerate() {
        let visivel = nome.chars().count() + info.eh_diretorio() as usize;
        linha.push_str(&colorido(nome, info));
        if (i + 1) % por_linha == 0 || i + 1 == itens.len() {
            println!("{}", linha);
            linha.clear();
        } else {
            linha.push_str(&" ".repeat(largura - visivel));
        }
    }
}

/// 0o4755 → "rwsr-xr-x", como o ls do Unix.
fn texto_permissoes(modo: u32) -> String {
    let mut t = String::new();
    for (i, especial, letra_especial) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let trio = (modo >> i) & 7;
        t.push(if trio & 4 != 0 { 'r' } else { '-' });
        t.push(if trio & 2 != 0 { 'w' } else { '-' });
        t.push(match (trio & 1 != 0, modo & especial != 0) {
            (true, true) => letra_especial,
            (false, true) => letra_especial.to_ascii_uppercase(),
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    t
}
