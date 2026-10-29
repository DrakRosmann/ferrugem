//! id — mostra a identidade do processo: uid e gid reais e efetivos.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios::{nome_do_grupo, nome_do_usuario};
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let (uid, gid, euid, egid) = (sys::uid(), sys::gid(), sys::euid(), sys::egid());
    let mut texto = format!("uid={}({}) gid={}({})", uid, nome_do_usuario(uid), gid, nome_do_grupo(gid));
    if euid != uid {
        texto.push_str(&format!(" euid={}({})", euid, nome_do_usuario(euid)));
    }
    if egid != gid {
        texto.push_str(&format!(" egid={}({})", egid, nome_do_grupo(egid)));
    }
    println!("{}", texto);
    0
}
