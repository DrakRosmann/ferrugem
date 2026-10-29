//! mount — monta um sistema de arquivos. Sem argumentos, lista o que está montado.
//!
//! ```text
//!   mount                         (lista, como o /proc/mounts)
//!   mount /dev/hdb1 /mnt          (ext2)
//!   mount -t tmpfs tmpfs /mnt
//!   mount -t proc proc /mnt
//! ```

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    if args.len() == 1 {
        match sys::ler_arquivo("/proc/mounts") {
            Ok(dados) => {
                for linha in String::from_utf8_lossy(&dados).lines() {
                    let c: Vec<&str> = linha.split(' ').collect();
                    if c.len() >= 3 {
                        println!("{} em {} tipo {}", c[0], c[1], c[2]);
                    }
                }
                return 0;
            }
            Err(e) => {
                eprintln!("mount: /proc/mounts: {}", e);
                return 1;
            }
        }
    }
    let (tipo, resto) = match args.get(1) {
        Some(&"-t") if args.len() >= 5 => (args[2], &args[3..]),
        _ => ("ext2", &args[1..]),
    };
    let [fonte, alvo] = resto[..] else {
        eprintln!("uso: mount [-t tipo] <fonte> <diretório>");
        return 2;
    };
    match sys::montar(fonte, alvo, tipo) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mount: {} em {}: {}", fonte, alvo, e);
            1
        }
    }
}
