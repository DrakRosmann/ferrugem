//! rm [-r] caminho...: apaga arquivos (e diretórios, com -r).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn remove(path: &str, recursive: bool) -> sys::Result<()> {
    let info = sys::stat(path)?;
    if !info.is_dir() {
        return sys::unlink(path);
    }
    if !recursive {
        return Err(21); // EISDIR
    }
    let fd = sys::open(path, sys::O_RDONLY | sys::O_DIRECTORY, 0)?;
    let names = sys::read_dir(fd)?;
    let _ = sys::close(fd);
    for name in names.iter().filter(|n| *n != "." && *n != "..") {
        remove(&format!("{}/{name}", path.trim_end_matches('/')), true)?;
    }
    sys::rmdir(path)
}

fn main() -> i32 {
    let mut recursive = false;
    let mut status = 0;
    for arg in rt::args().skip(1) {
        if arg == "-r" || arg == "-rf" {
            recursive = true;
            continue;
        }
        if let Err(errno) = remove(arg, recursive) {
            eprintln!("rm: {arg}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
