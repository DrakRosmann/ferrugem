//! Programas com janela: o lado do cliente do servidor de janelas (`janelas`).
//!
//! O programa se conecta ao socket Unix `/tmp/.janelas` e pede uma janela.
//! O servidor cria um segmento de memória compartilhada do tamanho da
//! imagem e responde com o número dele; o programa o anexa e desenha direto
//! nos pixels — sem copiar nada pelo socket. Depois de desenhar, avisa
//! ("atualizar") e o servidor mostra a imagem nova. Pelo mesmo socket chegam
//! os eventos (teclas, cliques, o botão de fechar).
//!
//! ```text
//!   cliente → servidor:  janela 300 200 Título\n     atualizar\n
//!   servidor → cliente:  ok 12\n  tecla 97\n  botao 10 20 1 1\n  mover 11 21\n  fechar\n
//! ```

use crate::grafico::Quadro;
use crate::io::Leitor;
use crate::sys;
use alloc::format;
use alloc::string::String;

pub const SOCKET: &str = "/tmp/.janelas";

pub struct Janela {
    fd: i32,
    pixels: usize,
    pub largura: i32,
    pub altura: i32,
    leitor: Leitor,
}

pub enum Evento {
    Tecla(char),
    /// botao: 1 = esquerdo, 2 = direito; apertado: true ao apertar, false ao soltar.
    Botao { x: i32, y: i32, botao: u8, apertado: bool },
    /// O mouse andou com algum botão apertado dentro da janela.
    Mover { x: i32, y: i32 },
    /// Clicaram no "x" da janela.
    Fechar,
}

impl Janela {
    pub fn abrir(largura: i32, altura: i32, titulo: &str) -> Result<Janela, String> {
        let fd = sys::socket(sys::SOCK_STREAM).map_err(|e| format!("socket: {}", e))?;
        sys::fechar_no_exec(fd);
        sys::conectar(fd, SOCKET).map_err(|e| format!("{}: {} (o `janelas` está rodando?)", SOCKET, e))?;
        sys::escrever_tudo(fd, format!("janela {} {} {}\n", largura, altura, titulo).as_bytes())
            .map_err(|e| format!("{}", e))?;
        let mut leitor = Leitor::new(fd);
        let resposta = leitor.ler_linha().ok().flatten().unwrap_or_default();
        let Some(id) = resposta.strip_prefix("ok ").and_then(|n| n.trim().parse().ok()) else {
            return Err(format!("o servidor recusou: {}", resposta));
        };
        let pixels = sys::shmat(id).map_err(|e| format!("shmat: {}", e))?;
        Ok(Janela { fd, pixels, largura, altura, leitor })
    }

    /// A imagem da janela, para desenhar.
    pub fn quadro(&mut self) -> Quadro<'_> {
        let n = (self.largura * self.altura) as usize;
        let pixels = unsafe { core::slice::from_raw_parts_mut(self.pixels as *mut u32, n) };
        Quadro { pixels, largura: self.largura, altura: self.altura }
    }

    /// Pede ao servidor para mostrar o que foi desenhado.
    pub fn mostrar(&mut self) {
        let _ = sys::escrever_tudo(self.fd, b"atualizar\n");
    }

    /// O descritor do socket (para esperar com `poll` junto com outras coisas).
    pub fn descritor(&self) -> i32 {
        self.fd
    }

    /// Espera o próximo evento. `None`: o servidor fechou.
    pub fn evento(&mut self) -> Option<Evento> {
        loop {
            let linha = self.leitor.ler_linha().ok()??;
            let mut partes = linha.split(' ');
            let comando = partes.next();
            let mut numero = || partes.next().and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
            let evento = match comando {
                Some("tecla") => char::from_u32(numero() as u32).map(Evento::Tecla),
                Some("botao") => {
                    let (x, y, b, a) = (numero(), numero(), numero(), numero());
                    Some(Evento::Botao { x, y, botao: b as u8, apertado: a == 1 })
                }
                Some("mover") => {
                    let (x, y) = (numero(), numero());
                    Some(Evento::Mover { x, y })
                }
                Some("fechar") => Some(Evento::Fechar),
                _ => None,
            };
            if evento.is_some() {
                return evento;
            }
        }
    }
}

impl Drop for Janela {
    fn drop(&mut self) {
        let _ = sys::shmdt(self.pixels);
        let _ = sys::fechar(self.fd);
    }
}
