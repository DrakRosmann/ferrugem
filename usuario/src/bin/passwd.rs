//! passwd — troca a senha. Setuid root, para poder gravar no /etc/shadow.
//! Um usuário comum só troca a própria senha, e precisa saber a atual.

#![no_std]
#![no_main]

use ulib::io;
use ulib::sys;
use ulib::usuarios;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let real = sys::uid();
    let nome = match args.get(1) {
        Some(n) => String::from(*n),
        None => usuarios::nome_do_usuario(real),
    };
    let Some(alvo) = usuarios::por_nome(&nome) else {
        eprintln!("passwd: usuário {} não existe", nome);
        return 1;
    };
    if real != 0 && alvo.uid != real {
        eprintln!("passwd: só o root troca a senha de outro usuário");
        return 1;
    }
    println!("Trocando a senha de {}.", nome);
    if real != 0 {
        let atual = io::ler_senha("Senha atual: ").unwrap_or_default();
        if !usuarios::conferir_senha(&nome, &atual) {
            let _ = sys::dormir_ms(1000);
            eprintln!("passwd: senha incorreta");
            return 1;
        }
    }
    let nova = io::ler_senha("Nova senha: ").unwrap_or_default();
    let repetida = io::ler_senha("Repita a nova senha: ").unwrap_or_default();
    if nova.is_empty() || nova != repetida {
        eprintln!("passwd: as senhas não conferem (ou estão vazias)");
        return 1;
    }
    match usuarios::trocar_senha(&nome, &nova) {
        Ok(()) => {
            println!("passwd: senha alterada (vale até desligar: o /etc fica na memória)");
            0
        }
        Err(e) => {
            eprintln!("passwd: {}", e);
            1
        }
    }
}
