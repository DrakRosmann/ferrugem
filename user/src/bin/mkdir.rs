//! mkdir [-p] diretório...: cria diretórios. Com -p, cria também os
//! diretórios intermediários e não reclama dos que já existem.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn make_parents(path: &str) -> sys::Result<()> {
    let mut partial = String::from(if path.starts_with('/') { "/" } else { "" });
    for component in path.split('/').filter(|c| !c.is_empty()) {
        if !partial.is_empty() && !partial.ends_with('/') {
            partial.push('/');
        }
        partial.push_str(component);
        if !sys::stat(&partial).is_ok_and(|info| info.is_dir()) {
            sys::mkdir(&partial)?;
        }
    }
    Ok(())
}

fn main() -> i32 {
    let mut status = 0;
    let mut parents = false;
    for path in rt::args().skip(1) {
        if path == "-p" {
            parents = true;
            continue;
        }
        let result = if parents { make_parents(path) } else { sys::mkdir(path) };
        if let Err(errno) = result {
            eprintln!("mkdir: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
