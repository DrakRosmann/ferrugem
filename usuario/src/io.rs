//! Entrada e saída: `print!`, `println!`, `eprintln!` e um leitor de linhas.

use crate::sys::{self, Erro};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

pub const ENTRADA: i32 = 0;
pub const SAIDA: i32 = 1;
pub const ERRO: i32 = 2;

#[doc(hidden)]
pub fn _imprimir(fd: i32, args: fmt::Arguments) {
    // Formata tudo antes: uma chamada `write` por `print!`, e não uma por pedaço.
    let texto = alloc::fmt::format(args);
    let _ = sys::escrever_tudo(fd, texto.as_bytes());
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::_imprimir($crate::io::SAIDA, format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => ($crate::io::_imprimir($crate::io::ERRO, format_args!($($arg)*)));
}

#[macro_export]
macro_rules! eprintln {
    () => ($crate::eprint!("\n"));
    ($($arg:tt)*) => ($crate::eprint!("{}\n", format_args!($($arg)*)));
}

/// Lê um descritor com buffer, linha a linha.
pub struct Leitor {
    fd: i32,
    buffer: Vec<u8>,
    fim: bool,
}

impl Leitor {
    pub fn new(fd: i32) -> Self {
        Self { fd, buffer: Vec::new(), fim: false }
    }

    /// Próxima linha, sem o `\n`. `Ok(None)` no fim do arquivo.
    pub fn ler_linha(&mut self) -> Result<Option<String>, Erro> {
        loop {
            if let Some(i) = self.buffer.iter().position(|&b| b == b'\n') {
                let linha: Vec<u8> = self.buffer.drain(..=i).collect();
                return Ok(Some(String::from_utf8_lossy(&linha[..i]).into_owned()));
            }
            if self.fim {
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                let resto = core::mem::take(&mut self.buffer);
                return Ok(Some(String::from_utf8_lossy(&resto).into_owned()));
            }
            let mut pedaco = [0u8; 1024];
            match sys::ler(self.fd, &mut pedaco) {
                Ok(0) => self.fim = true,
                Ok(n) => self.buffer.extend_from_slice(&pedaco[..n]),
                Err(e) => {
                    self.buffer.clear();
                    return Err(e);
                }
            }
        }
    }

    /// Já há uma linha inteira no buffer (ler_linha não vai esperar)?
    pub fn tem_linha(&self) -> bool {
        self.buffer.contains(&b'\n')
    }

    /// Reabre o leitor depois de um Ctrl+D no terminal.
    pub fn continuar(&mut self) {
        self.fim = false;
    }
}

/// Pergunta uma senha sem mostrar o que é digitado.
pub fn ler_senha(pergunta: &str) -> Option<String> {
    crate::print!("{}", pergunta);
    let _ = sys::eco(ENTRADA, false);
    let senha = Leitor::new(ENTRADA).ler_linha();
    let _ = sys::eco(ENTRADA, true);
    crate::println!();
    senha.ok().flatten()
}
