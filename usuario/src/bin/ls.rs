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
        // Como no ls do Unix: `ls -l link` mostra o link; `ls link-para-diretório`
        // mostra o que tem dentro do diretório.
        let info = match sys::lstat(caminho).and_then(|i| {
            let segue = i.eh_link() && !longo && sys::stat(caminho).is_ok_and(|alvo| alvo.eh_diretorio());
            if segue { sys::stat(caminho) } else { Ok(i) }
        }) {
            Ok(info) => info,
            Err(e) => {
                eprintln!("ls: {}: {}", caminho, e);
                status = 1;
                continue;
            }
        };
        if !info.eh_diretorio() {
            mostrar(&[(completo_e_nome(String::from(caminho), String::from(caminho)), info)], longo);
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
                        sys::lstat(&completo).ok().map(|s| (completo_e_nome(completo, e.nome), s))
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

/// Guardamos o caminho completo junto com o nome (separados por um \0) para
/// o `ls -l` conseguir ler para onde um link aponta.
fn completo_e_nome(completo: String, nome: String) -> String {
    format!("{}\0{}", nome, completo)
}

fn nome_visivel(item: &str) -> &str {
    item.split('\0').next().unwrap_or(item)
}

fn colorido(item: &str, info: &sys::Stat) -> String {
    let nome = nome_visivel(item);
    if info.eh_link() {
        format!("\x1b[96m{}\x1b[0m", nome)
    } else if info.eh_diretorio() {
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
            let tipo = if info.eh_link() {
                'l'
            } else if info.eh_diretorio() {
                'd'
            } else if info.eh_dispositivo() {
                'c'
            } else if info.eh_fifo() {
                'p'
            } else {
                '-'
            };
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
                "{}{} {:<8} {:<8} {:>7}  {}  {}{}",
                tipo,
                permissoes,
                dono,
                grupo,
                tamanho,
                &data[..data.len() - 3],
                colorido(nome, info),
                destino_do_link(nome, info)
            );
        }
        return;
    }
    // Para um pipe ou arquivo, um nome por linha (é o que `ls | wc -l` espera);
    // para o terminal, em colunas.
    if !sys::eh_terminal(1) {
        for (nome, _) in itens {
            println!("{}", nome_visivel(nome));
        }
        return;
    }
    // Em colunas, como o ls de verdade
    let largura = itens.iter().map(|(n, i)| nome_visivel(n).chars().count() + i.eh_diretorio() as usize).max().unwrap_or(0) + 2;
    let por_linha = (78 / largura.max(1)).max(1);
    let mut linha = String::new();
    for (i, (nome, info)) in itens.iter().enumerate() {
        let visivel = nome_visivel(nome).chars().count() + info.eh_diretorio() as usize;
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

/// " -> alvo", para links no `ls -l`.
fn destino_do_link(item: &str, info: &sys::Stat) -> String {
    if !info.eh_link() {
        return String::new();
    }
    let caminho = item.split('\0').nth(1).unwrap_or(item);
    match sys::ler_link(caminho) {
        Ok(alvo) => format!(" -> {}", alvo),
        Err(_) => String::new(),
    }
}
