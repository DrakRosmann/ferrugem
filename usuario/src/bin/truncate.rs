//! truncate — muda o tamanho de arquivos: `truncate -s 100 arquivo` (ou -s 0).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let (Some(&"-s"), Some(tamanho)) = (args.get(1), args.get(2).and_then(|t| t.parse::<u64>().ok())) else {
        eprintln!("uso: truncate -s <tamanho> <arquivos...>");
        return 2;
    };
    let mut status = 0;
    for &arquivo in &args[3..] {
        let r = match sys::truncar(arquivo, tamanho) {
            Err(e) if e == sys::ENOENT => sys::abrir(arquivo, sys::O_WRONLY | sys::O_CREAT)
                .and_then(|fd| {
                    let r = sys::truncar_fd(fd, tamanho);
                    let _ = sys::fechar(fd);
                    r
                }),
            outro => outro,
        };
        if let Err(e) = r {
            eprintln!("truncate: {}: {}", arquivo, e);
            status = 1;
        }
    }
    status
}
