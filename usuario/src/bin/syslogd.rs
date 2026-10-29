//! syslogd — recebe as mensagens dos programas e as grava em /var/log/mensagens.
//!
//! ```text
//!   syslogd        (vira um daemon: roda em segundo plano)
//!   syslogd -n     (fica em primeiro plano)
//! ```
//!
//! Os programas não escrevem no arquivo de registro diretamente: eles mandam
//! um datagrama para o socket Unix `/dev/log` (é o que a função `syslog()` da
//! libc faz), e o syslogd, o único que escreve no arquivo, acrescenta uma
//! linha por mensagem. A mensagem chega assim:
//!
//! ```text
//!   <38>Oct 29 17:05:09 login[12]: root entrou no tty1
//!    ^^ prioridade = facilidade * 8 + gravidade (38 = auth, info)
//! ```

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

const SOCKET: &str = "/dev/log";
const ARQUIVO: &str = "/var/log/mensagens";

/// Separa "<38>Oct 29 17:05:09 resto" em (prioridade, data, resto).
fn decifrar(bruto: &str) -> (u8, String, &str) {
    let mut texto = bruto.trim_end_matches(['\n', '\0']);
    let mut prioridade = sys::LOG_USER | sys::LOG_NOTICE;
    if let Some(resto) = texto.strip_prefix('<') {
        if let Some((numero, depois)) = resto.split_once('>') {
            if let Ok(p) = numero.parse() {
                prioridade = p;
                texto = depois;
            }
        }
    }
    // "Mmm dd hh:mm:ss " — se o programa já mandou a data, ela é mantida
    let b = texto.as_bytes();
    if b.len() > 16 && b[3] == b' ' && b[6] == b' ' && b[9] == b':' && b[12] == b':' && b[15] == b' ' {
        return (prioridade, String::from(&texto[..15]), &texto[16..]);
    }
    (prioridade, sys::formatar_data_syslog(sys::hora_atual()), texto)
}

fn principal(args: &[&str]) -> i32 {
    let primeiro_plano = args.get(1) == Some(&"-n");
    let _ = sys::remover(SOCKET); // sobra de uma execução anterior
    let fd = match sys::socket(sys::SOCK_DGRAM).and_then(|fd| sys::ligar(fd, SOCKET).map(|_| fd)) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("syslogd: {}: {}", SOCKET, e);
            return 1;
        }
    };
    // Qualquer um pode mandar mensagens (conectar a um socket exige escrita nele).
    let _ = sys::mudar_permissoes(SOCKET, 0o666);
    let _ = sys::criar_diretorio("/var/log");
    let arquivo = match sys::abrir_com_permissoes(ARQUIVO, sys::O_WRONLY | sys::O_CREAT | sys::O_APPEND, 0o640) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("syslogd: {}: {}", ARQUIVO, e);
            return 1;
        }
    };
    if !primeiro_plano {
        sys::daemonizar();
    }
    let maquina = sys::uname().maquina;
    let gravar = |data: &str, texto: &str| {
        let linha = format!("{} {} {}\n", data, maquina, texto);
        let _ = sys::escrever_tudo(arquivo, linha.as_bytes());
    };
    gravar(&sys::formatar_data_syslog(sys::hora_atual()), &format!("syslogd[{}]: iniciado", sys::pid()));

    let mut buf = vec![0u8; 4096];
    loop {
        match sys::ler(fd, &mut buf) {
            Ok(n) => {
                let bruto = String::from_utf8_lossy(&buf[..n]);
                let (prioridade, data, texto) = decifrar(&bruto);
                gravar(&data, texto);
                // Erros graves também aparecem no console.
                if prioridade & 7 <= sys::LOG_ERR {
                    if let Ok(console) = sys::abrir("/dev/tty1", sys::O_WRONLY | sys::O_NOCTTY) {
                        let _ = sys::escrever_tudo(console, format!("\n{} {}\n", data, texto).as_bytes());
                        let _ = sys::fechar(console);
                    }
                }
            }
            Err(sys::EINTR) => {}
            Err(_) => return 1,
        }
    }
}
