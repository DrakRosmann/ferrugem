//! chown — muda o dono (e, opcionalmente, o grupo): `chown usuario[:grupo] arquivos...`
//! Só o root pode.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    if args.len() < 3 {
        eprintln!("uso: chown <usuário>[:<grupo>] <arquivos...>");
        return 2;
    }
    let (dono, grupo) = args[1].split_once(':').unwrap_or((args[1], ""));
    let uid = match dono {
        "" => None,
        d => match d.parse().ok().or_else(|| usuarios::por_nome(d).map(|u| u.uid)) {
            Some(u) => Some(u),
            None => {
                eprintln!("chown: usuário {} não existe", d);
                return 1;
            }
        },
    };
    let gid = match grupo {
        "" => None,
        g => match g.parse().ok().or_else(|| usuarios::gid_do_grupo(g)) {
            Some(g) => Some(g),
            None => {
                eprintln!("chown: grupo {} não existe", g);
                return 1;
            }
        },
    };
    let mut status = 0;
    for &arquivo in &args[2..] {
        if let Err(e) = sys::mudar_dono(arquivo, uid, gid) {
            eprintln!("chown: {}: {}", arquivo, e);
            status = 1;
        }
    }
    status
}
