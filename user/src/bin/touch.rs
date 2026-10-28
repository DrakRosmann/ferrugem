//! touch arquivo...: cria arquivos vazios ou marca-os como modificados agora.

#![no_std]
#![no_main]

use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn main() -> i32 {
    let mut status = 0;
    for path in rt::args().skip(1) {
        let result = sys::open(path, sys::O_WRONLY | sys::O_CREAT, 0o644)
            .and_then(sys::close)
            .and_then(|()| sys::touch_now(path));
        if let Err(errno) = result {
            eprintln!("touch: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
