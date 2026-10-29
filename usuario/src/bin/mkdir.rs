//! mkdir — cria diretórios. `-p`: cria os intermediários e não reclama se já existir.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let pais = args.contains(&"-p");
    let mut status = 0;
    for &caminho in args[1..].iter().filter(|a| !a.starts_with('-')) {
        let resultado = if pais {
            let mut atual = String::from(if caminho.starts_with('/') { "/" } else { "" });
            let mut r = Ok(());
            for parte in caminho.split('/').filter(|p| !p.is_empty()) {
                atual.push_str(parte);
                r = match sys::criar_diretorio(&atual) {
                    Err(e) if e == sys::EEXIST => Ok(()),
                    outro => outro,
                };
                atual.push('/');
            }
            r
        } else {
            sys::criar_diretorio(caminho)
        };
        if let Err(e) = resultado {
            eprintln!("mkdir: {}: {}", caminho, e);
            status = 1;
        }
    }
    status
}
