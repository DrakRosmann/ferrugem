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
//!                        copiar <texto>\n   colar\n          (a área de transferência)
//!   servidor → cliente:  ok 12\n  tecla 97\n  seta A\n  botao 10 20 1 1\n  mover 11 21\n
//!                        roda 11 21 -1\n  fechar\n  colar <texto>\n
//!                        tamanho 400 300 13\n   (redimensionada: a imagem nova é o segmento 13)
//!
//! O texto da área de transferência vai numa linha só: `\\` vira `\\\\` e a
//! quebra de linha vira `\\n`.
//! ```

use crate::grafico::Quadro;
use crate::io::Leitor;
use crate::sys;
use alloc::format;
use alloc::string::String;

pub const SOCKET: &str = "/tmp/.janelas";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seta {
    Cima,
    Baixo,
    Direita,
    Esquerda,
}

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
    /// O mouse andou dentro da janela (ou fora dela, com um botão apertado
    /// desde que estava dentro).
    Mover { x: i32, y: i32 },
    /// A roda do mouse girou: negativo para cima, positivo para baixo.
    Roda { x: i32, y: i32, giro: i32 },
    /// Uma seta do teclado.
    Seta(Seta),
    /// Clicaram no "x" da janela.
    Fechar,
    /// A janela mudou de tamanho: a imagem nova (`quadro()`) está em branco.
    Tamanho { largura: i32, altura: i32 },
    /// A resposta de `pedir_colagem`: o texto da área de transferência.
    Colar(String),
}

/// Põe o texto numa linha só, para o protocolo.
pub fn escapar(texto: &str) -> String {
    let mut s = String::with_capacity(texto.len());
    for c in texto.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            c => s.push(c),
        }
    }
    s
}

pub fn desescapar(texto: &str) -> String {
    let mut s = String::with_capacity(texto.len());
    let mut letras = texto.chars();
    while let Some(c) = letras.next() {
        if c == '\\' {
            match letras.next() {
                Some('n') => s.push('\n'),
                Some(o) => s.push(o),
                None => {}
            }
        } else {
            s.push(c);
        }
    }
    s
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
        Quadro::novo(pixels, self.largura, self.altura)
    }

    /// Pede ao servidor para mostrar o que foi desenhado.
    pub fn mostrar(&mut self) {
        let _ = sys::escrever_tudo(self.fd, b"atualizar\n");
    }

    /// Põe um texto na área de transferência (a que todos os programas dividem).
    pub fn copiar(&mut self, texto: &str) {
        let _ = sys::escrever_tudo(self.fd, format!("copiar {}\n", escapar(texto)).as_bytes());
    }

    /// Pede o texto da área de transferência: ele chega como `Evento::Colar`.
    pub fn pedir_colagem(&mut self) {
        let _ = sys::escrever_tudo(self.fd, b"colar\n");
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
                Some("roda") => {
                    let (x, y, giro) = (numero(), numero(), numero());
                    Some(Evento::Roda { x, y, giro })
                }
                Some("seta") => match partes.next() {
                    Some("A") => Some(Evento::Seta(Seta::Cima)),
                    Some("B") => Some(Evento::Seta(Seta::Baixo)),
                    Some("C") => Some(Evento::Seta(Seta::Direita)),
                    Some("D") => Some(Evento::Seta(Seta::Esquerda)),
                    _ => None,
                },
                Some("fechar") => Some(Evento::Fechar),
                Some("colar") => Some(Evento::Colar(desescapar(linha.get(6..).unwrap_or("")))),
                Some("tamanho") => {
                    let (largura, altura, id) = (numero(), numero(), numero());
                    match sys::shmat(id) {
                        Ok(novos) if largura > 0 && altura > 0 => {
                            let _ = sys::shmdt(self.pixels);
                            (self.pixels, self.largura, self.altura) = (novos, largura, altura);
                            Some(Evento::Tamanho { largura, altura })
                        }
                        _ => None,
                    }
                }
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
