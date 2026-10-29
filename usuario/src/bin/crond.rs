//! crond — executa tarefas agendadas (o "cron" do Unix).
//!
//! A cada minuto, confere as tabelas de tarefas e roda as que combinam com a
//! hora atual:
//!
//! ```text
//!   /etc/crontab                 do sistema: diz o usuário de cada tarefa
//!   /var/spool/cron/<usuário>    de cada usuário (editadas com o `crontab`)
//!
//!   # minuto hora dia mês dia-da-semana [usuário] comando
//!   */5  *   *   *   *      root  sync
//!   0    3   *   *   1-5    root  logger backup das 3h (dias úteis)
//!   @reboot                 root  logger -t cron o sistema ligou
//! ```
//!
//! Cada campo aceita `*`, números, listas (`1,15`), faixas (`1-5`) e passos
//! (`*/10`, `0-30/5`). Dia da semana: 0 (ou 7) é domingo. As horas são UTC.
//! O que a tarefa escrever vai para o registro do sistema (/var/log/mensagens).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios;
use ulib::*;

programa!(principal);

const DIRETORIO: &str = "/var/spool/cron";

struct Tarefa {
    /// Bits dos valores permitidos de cada campo: minuto, hora, dia, mês, dia da semana.
    campos: [u64; 5],
    dia_livre: bool,
    semana_livre: bool,
    no_boot: bool,
    usuario: String,
    comando: String,
}

/// Um campo ("*/5", "1-5", "0,30") vira um conjunto de bits.
fn campo(texto: &str, minimo: u32, maximo: u32) -> Option<u64> {
    let mut bits = 0u64;
    for parte in texto.split(',') {
        let (faixa, passo) = match parte.split_once('/') {
            Some((f, p)) => (f, p.parse::<u32>().ok().filter(|&p| p > 0)?),
            None => (parte, 1),
        };
        let (de, ate) = if faixa == "*" {
            (minimo, maximo)
        } else if let Some((a, b)) = faixa.split_once('-') {
            (a.parse().ok()?, b.parse().ok()?)
        } else {
            let v = faixa.parse().ok()?;
            (v, if passo > 1 { maximo } else { v })
        };
        if de < minimo || ate > maximo || de > ate {
            return None;
        }
        for v in (de..=ate).step_by(passo as usize) {
            bits |= 1 << v;
        }
    }
    Some(bits)
}

/// Uma linha da tabela. `usuario`: Some se a tabela é de um usuário (sem a coluna).
fn ler_linha(linha: &str, usuario: Option<&str>) -> Option<Tarefa> {
    let linha = linha.trim();
    if linha.is_empty() || linha.starts_with('#') {
        return None;
    }
    let mut palavras = linha.split_whitespace();
    let primeiro = palavras.next()?;
    let atalho = match primeiro {
        "@reboot" => Some(None),
        "@hourly" => Some(Some("0 * * * *")),
        "@daily" | "@midnight" => Some(Some("0 0 * * *")),
        "@weekly" => Some(Some("0 0 * * 0")),
        "@monthly" => Some(Some("0 0 1 * *")),
        "@yearly" | "@annually" => Some(Some("0 0 1 1 *")),
        _ => None,
    };
    let (textos, no_boot): (Vec<&str>, bool) = match atalho {
        Some(Some(t)) => (t.split(' ').collect(), false),
        Some(None) => (Vec::from(["*"; 5]), true),
        None => {
            let mut v = Vec::from([primeiro]);
            for _ in 0..4 {
                v.push(palavras.next()?);
            }
            (v, false)
        }
    };
    let limites = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
    let mut campos = [0u64; 5];
    for i in 0..5 {
        campos[i] = campo(textos[i], limites[i].0, limites[i].1)?;
    }
    if campos[4] & (1 << 7) != 0 {
        campos[4] |= 1; // 7 também é domingo
    }
    let usuario = match usuario {
        Some(u) => String::from(u),
        None => String::from(palavras.next()?),
    };
    let comando = palavras.collect::<Vec<_>>().join(" ");
    if comando.is_empty() {
        return None;
    }
    Some(Tarefa { campos, dia_livre: textos[2] == "*", semana_livre: textos[4] == "*", no_boot, usuario, comando })
}

impl Tarefa {
    fn combina(&self, d: &sys::Data) -> bool {
        let tem = |i: usize, v: u32| self.campos[i] & (1 << v) != 0;
        if self.no_boot || !tem(0, d.minuto) || !tem(1, d.hora) || !tem(3, d.mes) {
            return false;
        }
        // Regra clássica: se o dia do mês e o da semana forem restritos, basta um.
        let dia = tem(2, d.dia);
        let semana = tem(4, d.dia_da_semana);
        match (self.dia_livre, self.semana_livre) {
            (false, false) => dia || semana,
            _ => dia && semana,
        }
    }
}

fn carregar() -> Vec<Tarefa> {
    let texto = |caminho: &str| String::from_utf8_lossy(&sys::ler_arquivo(caminho).unwrap_or_default()).into_owned();
    let mut tarefas: Vec<Tarefa> = texto("/etc/crontab").lines().filter_map(|l| ler_linha(l, None)).collect();
    for entrada in sys::listar_diretorio(DIRETORIO).unwrap_or_default() {
        if entrada.nome.starts_with('.') || usuarios::por_nome(&entrada.nome).is_none() {
            continue;
        }
        let caminho = format!("{}/{}", DIRETORIO, entrada.nome);
        tarefas.extend(texto(&caminho).lines().filter_map(|l| ler_linha(l, Some(&entrada.nome))));
    }
    tarefas
}

/// Roda uma tarefa num processo à parte, que espera por ela e registra a saída.
fn executar(t: &Tarefa) {
    let Some(u) = usuarios::por_nome(&t.usuario) else {
        sys::registrar(sys::LOG_CRON | sys::LOG_WARNING, "cron", &format!("usuário desconhecido: {}", t.usuario));
        return;
    };
    // Fork duplo: o filho cria o executor e sai na hora (recolhido já aqui);
    // o executor, órfão, fica com o init, que o recolhe quando terminar.
    match sys::fork() {
        Ok(0) => {
            if sys::fork() != Ok(0) {
                sys::sair(0);
            }
        }
        Ok(filho) => {
            let _ = sys::esperar(filho);
            return;
        }
        Err(_) => return,
    }
    sys::registrar(sys::LOG_CRON | sys::LOG_INFO, "cron", &format!("({}) executando: {}", u.nome, t.comando));
    let Ok((leitura, escrita)) = sys::pipe() else { sys::sair(1) };
    let tarefa = match sys::fork() {
        Ok(0) => {
            let _ = sys::fechar(leitura);
            let _ = sys::duplicar_para(escrita, 1);
            let _ = sys::duplicar_para(escrita, 2);
            if sys::mudar_gid(u.gid).is_err() || sys::mudar_uid(u.uid).is_err() {
                sys::sair(126);
            }
            let _ = sys::mudar_diretorio(&u.casa);
            let ambiente = [
                format!("HOME={}", u.casa),
                format!("USER={}", u.nome),
                format!("LOGNAME={}", u.nome),
                format!("SHELL={}", u.shell),
                String::from("PATH=/bin"),
            ];
            let erro = sys::executar_com_ambiente("/bin/sh", &["sh", "-c", &t.comando], &ambiente);
            eprintln!("cron: /bin/sh: {}", erro);
            sys::sair(127);
        }
        Ok(pid) => pid,
        Err(_) => sys::sair(1),
    };
    let _ = sys::fechar(escrita);
    let mut leitor = io::Leitor::new(leitura);
    while let Ok(Some(linha)) = leitor.ler_linha() {
        sys::registrar(sys::LOG_CRON | sys::LOG_INFO, "cron", &format!("({}) {}", u.nome, linha.trim_end()));
    }
    if let Ok((_, status)) = sys::esperar(tarefa) {
        match sys::codigo_de_saida(status) {
            Some(0) => {}
            Some(c) => sys::registrar(sys::LOG_CRON | sys::LOG_NOTICE, "cron", &format!("({}) terminou com {}", u.nome, c)),
            None => sys::registrar(sys::LOG_CRON | sys::LOG_NOTICE, "cron", &format!("({}) morta por um sinal", u.nome)),
        }
    }
    sys::sair(0);
}

fn principal(args: &[&str]) -> i32 {
    if args.get(1) != Some(&"-n") {
        sys::daemonizar();
    }
    sys::registrar(sys::LOG_CRON | sys::LOG_INFO, "cron", "crond iniciado");
    for t in carregar().iter().filter(|t| t.no_boot) {
        executar(t);
    }
    let mut ultimo_minuto = sys::hora_atual() / 60;
    loop {
        // Dorme até o começo do próximo minuto.
        let agora = sys::hora_atual();
        let _ = sys::dormir_ms((60 - agora % 60) * 1000);
        let minuto = sys::hora_atual() / 60;
        if minuto == ultimo_minuto {
            continue; // acordou cedo (um sinal)
        }
        ultimo_minuto = minuto;
        let data = sys::data(minuto * 60);
        for t in carregar().iter().filter(|t| t.combina(&data)) {
            executar(t);
        }
    }
}
