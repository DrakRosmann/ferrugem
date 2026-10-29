//! O registro do kernel: as últimas mensagens que ele imprimiu (o `dmesg`).
//!
//! Tudo o que o kernel escreve com `println!` também é guardado aqui, num
//! buffer circular de 16 KiB (o mais antigo vai sendo sobrescrito), com o
//! instante em que foi escrito. Assim dá para ver as mensagens do boot
//! depois que elas já saíram da tela. Os programas leem com a chamada
//! `syslog` (a mesma do Linux), que é o que o `dmesg` usa.

use crate::pit;
use crate::spin::Mutex;
use alloc::vec::Vec;
use core::fmt;

const TAMANHO: usize = 16 * 1024;

struct Registro {
    dados: [u8; TAMANHO],
    /// Quantos bytes já foram escritos desde o boot (a posição é isso % TAMANHO).
    escritos: usize,
    inicio_de_linha: bool,
    dentro_de_escape: bool,
}

static REGISTRO: Mutex<Registro> =
    Mutex::new(Registro { dados: [0; TAMANHO], escritos: 0, inicio_de_linha: true, dentro_de_escape: false });

impl Registro {
    fn byte(&mut self, b: u8) {
        self.dados[self.escritos % TAMANHO] = b;
        self.escritos += 1;
    }
}

impl fmt::Write for Registro {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            // As cores (ESC [ ... m) ficam de fora: o registro é texto puro.
            if self.dentro_de_escape {
                self.dentro_de_escape = !b.is_ascii_alphabetic();
                continue;
            }
            if b == 0x1B {
                self.dentro_de_escape = true;
                continue;
            }
            if self.inicio_de_linha {
                // "[   12.345] " — montado na pilha: as primeiras mensagens do
                // boot saem antes de existir o heap.
                self.inicio_de_linha = false;
                let ms = pit::milissegundos();
                let mut carimbo = *b"[    0.000] ";
                let mut segundos = ms / 1000;
                for i in (1..=5).rev() {
                    carimbo[i] = b'0' + (segundos % 10) as u8;
                    segundos /= 10;
                    if segundos == 0 {
                        break;
                    }
                }
                carimbo[7] = b'0' + (ms % 1000 / 100) as u8;
                carimbo[8] = b'0' + (ms % 100 / 10) as u8;
                carimbo[9] = b'0' + (ms % 10) as u8;
                for c in carimbo {
                    self.byte(c);
                }
            }
            self.byte(b);
            if b == b'\n' {
                self.inicio_de_linha = true;
            }
        }
        Ok(())
    }
}

/// Para o pânico: ele pode ter acontecido com o registro travado.
/// # Safety
/// Quem segurava o lock não pode voltar a rodar.
pub unsafe fn destravar() {
    unsafe { REGISTRO.forcar_destravar() };
}

/// Chamada pelo `print!` do kernel (já sem interrupções).
pub fn guardar(args: fmt::Arguments) {
    let _ = fmt::Write::write_fmt(&mut *REGISTRO.lock(), args);
}

/// Tudo o que ainda está no buffer, do mais antigo ao mais novo.
pub fn conteudo() -> Vec<u8> {
    let r = REGISTRO.lock();
    if r.escritos <= TAMANHO {
        return r.dados[..r.escritos].to_vec();
    }
    let p = r.escritos % TAMANHO;
    let mut v = r.dados[p..].to_vec();
    v.extend_from_slice(&r.dados[..p]);
    v
}

/// A chamada `syslog(tipo, buf, tamanho)` do Linux (a "klogctl" da libc).
pub fn syscall(tipo: u64, buf: u64, tamanho: u64) -> crate::syscall::Resultado {
    const LER_TUDO: u64 = 3;
    const TAMANHO_DO_BUFFER: u64 = 10;
    match tipo {
        LER_TUDO => {
            let c = conteudo();
            let n = c.len().min(tamanho as usize);
            // os mais recentes, se não couber tudo
            crate::syscall::escrever_usuario(buf, &c[c.len() - n..])?;
            Ok(n as u64)
        }
        TAMANHO_DO_BUFFER => Ok(TAMANHO as u64),
        _ => Err(crate::erro::EINVAL),
    }
}
