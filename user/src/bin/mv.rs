//! mv origem... destino: move ou renomeia. Se o destino é um diretório, as
//! origens vão para dentro dele. Entre sistemas de arquivos diferentes (do
//! /tmp para o disco, por exemplo), arquivos são copiados e apagados.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

fn move_one(from: &str, to: &str) -> sys::Result<()> {
    match sys::rename(from, to) {
        Err(sys::EXDEV) => {
            sys::copy_file(from, to)?;
            sys::unlink(from)
        }
        result => result,
    }
}

fn main() -> i32 {
    let args: Vec<&str> = rt::args().skip(1).collect();
    let Some((&target, sources)) = args.split_last().filter(|(_, sources)| !sources.is_empty()) else {
        eprintln!("uso: mv origem... destino");
        return 2;
    };
    let into_dir = sys::stat(target).is_ok_and(|info| info.is_dir());
    if sources.len() > 1 && !into_dir {
        eprintln!("mv: {target}: não é um diretório");
        return 1;
    }
    let mut status = 0;
    for source in sources {
        let destination = if into_dir { format!("{}/{}", target.trim_end_matches('/'), basename(source)) } else { target.into() };
        if let Err(errno) = move_one(source, &destination) {
            eprintln!("mv: {source}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
