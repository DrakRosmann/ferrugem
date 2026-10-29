//! chmod — muda as permissões. Aceita octal (`chmod 755 x`) ou a forma
//! simbólica (`chmod u+x x`, `chmod go-w x`, `chmod a=r x`, `chmod +x x`).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

/// Aplica uma ou mais mudanças simbólicas separadas por vírgula.
fn simbolico(texto: &str, atual: u32) -> Option<u32> {
    let mut modo = atual;
    for parte in texto.split(',') {
        let pos = parte.find(['+', '-', '='])?;
        let (quem, resto) = parte.split_at(pos);
        let (operacao, bits) = resto.split_at(1);
        let quem = if quem.is_empty() { "a" } else { quem };
        let mut mascara = 0;
        for q in quem.chars() {
            mascara |= match q {
                'u' => 0o4700,
                'g' => 0o2070,
                'o' => 0o1007,
                'a' => 0o7777,
                _ => return None,
            };
        }
        let mut valor = 0;
        for b in bits.chars() {
            valor |= match b {
                'r' => 0o444,
                'w' => 0o222,
                'x' => 0o111,
                's' => 0o6000,
                't' => 0o1000,
                _ => return None,
            };
        }
        let valor = valor & mascara;
        modo = match operacao {
            "+" => modo | valor,
            "-" => modo & !valor,
            _ => (modo & !mascara) | valor,
        };
    }
    Some(modo)
}

fn principal(args: &[&str]) -> i32 {
    if args.len() < 3 {
        eprintln!("uso: chmod <modo> <arquivos...>   (ex.: 644, u+x, go-w)");
        return 2;
    }
    let mut status = 0;
    for &arquivo in &args[2..] {
        let resultado = sys::stat(arquivo).and_then(|info| {
            let atual = info.modo & 0o7777;
            let novo = u32::from_str_radix(args[1], 8).ok().or_else(|| simbolico(args[1], atual));
            match novo {
                Some(modo) => sys::mudar_permissoes(arquivo, modo),
                None => Err(sys::Erro(22)),
            }
        });
        if let Err(e) = resultado {
            eprintln!("chmod: {}: {}", arquivo, e);
            status = 1;
        }
    }
    status
}
