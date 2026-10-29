//! sh — o shell.
//!
//! Lê uma linha, quebra em palavras e operadores, e executa:
//!
//! ```text
//!   ls -l /bin                 programa com argumentos (procurado em /bin)
//!   ls | grep sh | wc -l       pipeline: a saída de um vira a entrada do próximo
//!   echo oi > a.txt            redireciona a saída (>> acrescenta no fim)
//!   wc < a.txt                 redireciona a entrada
//!   sleep 5 &                  roda em segundo plano
//!   cd /tmp ; ls               vários comandos numa linha
//!   echo $?                    código de saída do último comando
//!   ls ~/docs                  ~ é o diretório pessoal
//!   export NOME=valor; echo $NOME   variáveis de ambiente (passam para os programas)
//!   sh -c 'ls | wc -l'         roda uma linha (é o que system() e popen() do C usam)
//!   Ctrl+Z, jobs, fg, bg       controle de jobs
//!
//! ## Controle de jobs
//!
//! Cada pipeline vira um *grupo de processos* (um "job"), e o shell entrega o
//! terminal ao grupo que está em primeiro plano. Assim o Ctrl+C e o Ctrl+Z do
//! teclado chegam só nele. Ctrl+Z *para* o job (SIGTSTP); `fg` devolve o
//! terminal a ele e manda SIGCONT; `bg` o deixa continuar em segundo plano.
//!   'texto com espaços'        aspas simples ou duplas
//! ```
//!
//! Cada comando roda num processo novo: o shell faz `fork`, e o filho ajusta
//! os descritores 0 e 1 (com `dup2`) antes de chamar `execve`. É exatamente
//! assim que os shells do Unix funcionam desde os anos 70.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios;
use ulib::*;

programa!(principal);

/// O diretório pessoal de quem está usando o shell (para `cd` e para o `~`).
fn casa() -> String {
    if let Some(casa) = variavel("HOME") {
        return String::from(casa);
    }
    usuarios::por_uid(sys::uid()).map(|u| u.casa).unwrap_or_else(|| String::from("/"))
}

fn prompt() -> String {
    let uid = sys::uid();
    let usuario = usuarios::nome_do_usuario(uid);
    let mut dir = sys::diretorio_atual();
    let casa = casa();
    if casa != "/" && (dir == casa || dir.starts_with(&format!("{}/", casa))) {
        dir = format!("~{}", &dir[casa.len()..]);
    }
    let (cor, simbolo) = if uid == 0 { ("91", '#') } else { ("92", '$') };
    format!("\x1b[{}m{}@{}\x1b[0m:\x1b[94m{}\x1b[0m{} ", cor, usuario, sys::uname().maquina, dir, simbolo)
}

fn principal(args: &[&str]) -> i32 {
    // Ctrl+C mata o programa que está rodando, não o shell.
    sys::ignorar_sinal(sys::SIGINT, true);
    let mut shell = Shell {
        ultimo_status: 0,
        ambiente: ambiente().iter().map(|v| String::from(*v)).collect(),
        interativo: false,
        meu_grupo: sys::grupo(),
        jobs: Vec::new(),
    };

    // `sh -c "comando"`: é assim que system() e popen() da libc chamam o shell.
    if args.get(1) == Some(&"-c") {
        let Some(comando) = args.get(2) else {
            eprintln!("sh: -c precisa de um comando");
            return 2;
        };
        shell.executar_linha(comando);
        return shell.ultimo_status;
    }

    if let Some(script) = args.get(1) {
        let dados = match sys::ler_arquivo(script) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("sh: {}: {}", script, e);
                return 127;
            }
        };
        for linha in String::from_utf8_lossy(&dados).lines() {
            shell.executar_linha(linha);
        }
        return shell.ultimo_status;
    }

    // Shell interativo: um grupo só dele, dono do terminal, e imune aos sinais
    // de parar (quem para é o job, não o shell).
    shell.interativo = true;
    for sinal in [sys::SIGTSTP, sys::SIGTTIN, sys::SIGTTOU] {
        sys::ignorar_sinal(sinal, true);
    }
    let _ = sys::mudar_grupo(0, 0);
    shell.meu_grupo = sys::grupo();
    let _ = sys::dar_terminal(0, shell.meu_grupo);

    let mut entrada = Leitor::new(0);
    loop {
        shell.recolher_segundo_plano();
        print!("{}", prompt());
        match entrada.ler_linha() {
            Ok(Some(linha)) => shell.executar_linha(&linha),
            Ok(None) => {
                println!("exit");
                shell.sair(shell.ultimo_status);
            }
            Err(e) if e == sys::EINTR => {} // Ctrl+C: prompt novo
            Err(e) => {
                eprintln!("sh: erro lendo o terminal: {}", e);
                return 1;
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Análise da linha
// -----------------------------------------------------------------------------

enum Token {
    Palavra(String),
    Pipe,        // |
    Entrada,     // <
    Saida,       // >
    Acrescentar, // >>
    Fundo,       // &
    Separador,   // ;
}

/// `$NOME`: o nome vai até o primeiro caractere que não for letra, número ou _.
fn expandir_variavel(chars: &mut core::iter::Peekable<core::str::Chars>, status: i32, ambiente: &[String]) -> String {
    if chars.peek() == Some(&'?') {
        chars.next();
        return status.to_string();
    }
    let mut nome = String::new();
    while let Some(&c) = chars.peek() {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            break;
        }
        nome.push(c);
        chars.next();
    }
    if nome.is_empty() {
        return String::from("$");
    }
    let prefixo = format!("{}=", nome);
    ambiente.iter().find_map(|v| v.strip_prefix(prefixo.as_str())).map(String::from).unwrap_or_default()
}

/// Marca no lugar de um `$` que deve ser expandido. A expansão só acontece na
/// hora de executar cada comando: assim `export A=1; echo $A` vê o valor novo.
/// (Um `$` entre aspas simples continua sendo um `$` comum.)
const VARIAVEL: char = '\u{1}';

/// Troca cada marca de variável pelo valor dela.
fn expandir(palavra: &str, status: i32, ambiente: &[String]) -> String {
    let mut resultado = String::new();
    let mut chars = palavra.chars().peekable();
    while let Some(c) = chars.next() {
        if c == VARIAVEL {
            resultado.push_str(&expandir_variavel(&mut chars, status, ambiente));
        } else {
            resultado.push(c);
        }
    }
    resultado
}

fn tokenizar(linha: &str, casa: &str) -> Result<Vec<Token>, &'static str> {
    let mut tokens = Vec::new();
    let mut chars = linha.chars().peekable();
    while let Some(&c) = chars.peek() {
        let simples = match c {
            ' ' | '\t' => {
                chars.next();
                continue;
            }
            '#' => break, // comentário até o fim da linha
            '|' => Some(Token::Pipe),
            '<' => Some(Token::Entrada),
            '&' => Some(Token::Fundo),
            ';' => Some(Token::Separador),
            '>' => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Acrescentar);
                } else {
                    tokens.push(Token::Saida);
                }
                continue;
            }
            _ => None,
        };
        if let Some(token) = simples {
            chars.next();
            tokens.push(token);
            continue;
        }

        let mut palavra = String::new();
        // `~` no começo de uma palavra (sozinho ou antes de /) vira o diretório pessoal.
        if c == '~' {
            let mut adiante = chars.clone();
            adiante.next();
            if matches!(adiante.peek(), None | Some('/' | ' ' | '\t' | '|' | ';' | '&' | '<' | '>')) {
                chars.next();
                palavra.push_str(casa);
            }
        }
        while let Some(&c) = chars.peek() {
            match c {
                ' ' | '\t' | '|' | '<' | '>' | '&' | ';' => break,
                '\'' => {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('\'') => break,
                            Some(c) => palavra.push(c),
                            None => return Err("aspas simples sem fechar"),
                        }
                    }
                }
                '"' => {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some('$') => palavra.push(VARIAVEL),
                            Some(c) => palavra.push(c),
                            None => return Err("aspas duplas sem fechar"),
                        }
                    }
                }
                '$' => {
                    chars.next();
                    palavra.push(VARIAVEL);
                }
                _ => {
                    palavra.push(c);
                    chars.next();
                }
            }
        }
        tokens.push(Token::Palavra(palavra));
    }
    Ok(tokens)
}

#[derive(Default)]
struct Comando {
    args: Vec<String>,
    entrada: Option<String>,
    saida: Option<(String, bool)>, // (arquivo, acrescentar)
}

struct Pipeline {
    comandos: Vec<Comando>,
    fundo: bool,
}

fn analisar(tokens: Vec<Token>) -> Result<Vec<Pipeline>, &'static str> {
    let mut pipelines = Vec::new();
    let mut comandos: Vec<Comando> = Vec::new();
    let mut atual = Comando::default();
    let mut tokens = tokens.into_iter();

    while let Some(token) = tokens.next() {
        match token {
            Token::Palavra(p) => atual.args.push(p),
            Token::Entrada => match tokens.next() {
                Some(Token::Palavra(p)) => atual.entrada = Some(p),
                _ => return Err("faltou o arquivo depois de <"),
            },
            Token::Saida | Token::Acrescentar => {
                let acrescentar = matches!(token, Token::Acrescentar);
                match tokens.next() {
                    Some(Token::Palavra(p)) => atual.saida = Some((p, acrescentar)),
                    _ => return Err("faltou o arquivo depois de >"),
                }
            }
            Token::Pipe => {
                if atual.args.is_empty() {
                    return Err("comando vazio antes de |");
                }
                comandos.push(core::mem::take(&mut atual));
            }
            Token::Separador | Token::Fundo => {
                let fundo = matches!(token, Token::Fundo);
                if atual.args.is_empty() {
                    if !comandos.is_empty() {
                        return Err("comando vazio depois de |");
                    }
                    if fundo {
                        return Err("& sem comando");
                    }
                    continue;
                }
                comandos.push(core::mem::take(&mut atual));
                pipelines.push(Pipeline { comandos: core::mem::take(&mut comandos), fundo });
            }
        }
    }
    if !atual.args.is_empty() {
        comandos.push(atual);
    } else if !comandos.is_empty() {
        return Err("comando vazio depois de |");
    }
    if !comandos.is_empty() {
        pipelines.push(Pipeline { comandos, fundo: false });
    }
    Ok(pipelines)
}

// -----------------------------------------------------------------------------
// Execução
// -----------------------------------------------------------------------------

struct Shell {
    ultimo_status: i32,
    /// As variáveis de ambiente que os programas vão herdar ("NOME=valor").
    ambiente: Vec<String>,
    /// Com controle de jobs (no terminal) ou não (scripts e `sh -c`).
    interativo: bool,
    meu_grupo: i32,
    jobs: Vec<Job>,
}

#[derive(Clone, Copy, PartialEq)]
enum EstadoJob {
    Rodando,
    Parado,
}

/// Um pipeline que ainda não terminou: em segundo plano ou parado.
struct Job {
    numero: usize,
    pgid: i32,
    /// Os processos dele que ainda não terminaram.
    pids: Vec<i32>,
    texto: String,
    estado: EstadoJob,
}

fn texto_do_pipeline(p: &Pipeline) -> String {
    let partes: Vec<String> = p.comandos.iter().map(|c| c.args.join(" ")).collect();
    partes.join(" | ")
}

fn status_para_codigo(status: i32) -> i32 {
    match sys::sinal_que_matou(status) {
        Some(sinal) => 128 + sinal,
        None => sys::codigo_de_saida(status).unwrap_or(1),
    }
}

impl Shell {
    fn executar_linha(&mut self, linha: &str) {
        let pipelines = match tokenizar(linha, &casa()).and_then(analisar) {
            Ok(p) => p,
            Err(erro) => {
                eprintln!("sh: {}", erro);
                self.ultimo_status = 2;
                return;
            }
        };
        for pipeline in pipelines {
            self.executar_pipeline(pipeline);
        }
    }

    fn executar_pipeline(&mut self, mut p: Pipeline) {
        let (status, ambiente) = (self.ultimo_status, &self.ambiente);
        for comando in p.comandos.iter_mut() {
            for arg in comando.args.iter_mut() {
                *arg = expandir(arg, status, ambiente);
            }
            if let Some(arquivo) = comando.entrada.as_mut() {
                *arquivo = expandir(arquivo, status, ambiente);
            }
            if let Some((arquivo, _)) = comando.saida.as_mut() {
                *arquivo = expandir(arquivo, status, ambiente);
            }
        }
        // `cd`, `export` e `exit` precisam rodar no próprio shell: num filho, mudariam o filho.
        if p.comandos.len() == 1 && !p.fundo {
            if let Some(status) = self.variaveis(&p.comandos[0]) {
                self.ultimo_status = status;
                return;
            }
            if let Some(status) = self.controle_de_jobs(&p.comandos[0]) {
                self.ultimo_status = status;
                return;
            }
            if let Some(status) = interno(&p.comandos[0]) {
                self.ultimo_status = status;
                return;
            }
        }

        let n = p.comandos.len();
        let mut pids = Vec::new();
        let mut pgid = 0; // o grupo do job é o pid do primeiro processo
        let mut entrada_anterior: Option<i32> = None;
        for (i, comando) in p.comandos.iter().enumerate() {
            let pipe = if i + 1 < n {
                match sys::pipe() {
                    Ok(pipe) => Some(pipe),
                    Err(e) => {
                        eprintln!("sh: pipe: {}", e);
                        break;
                    }
                }
            } else {
                None
            };
            let grupo = self.interativo.then_some(pgid);
            match sys::fork() {
                Ok(0) => filho(comando, entrada_anterior, pipe, p.fundo, &self.ambiente, grupo),
                Ok(pid) => {
                    if self.interativo {
                        if pgid == 0 {
                            pgid = pid;
                        }
                        // O filho também faz isso; fazer nos dois evita depender de quem roda antes.
                        let _ = sys::mudar_grupo(pid, pgid);
                    }
                    pids.push(pid);
                }
                Err(e) => eprintln!("sh: fork: {}", e),
            }
            // O pai não usa as pontas dos pipes: fecha, senão o leitor nunca
            // receberia fim de arquivo (sempre haveria um escritor aberto).
            if let Some(fd) = entrada_anterior {
                let _ = sys::fechar(fd);
            }
            entrada_anterior = pipe.map(|(leitura, escrita)| {
                let _ = sys::fechar(escrita);
                leitura
            });
        }
        if let Some(fd) = entrada_anterior {
            let _ = sys::fechar(fd);
        }

        if pids.is_empty() {
            return;
        }
        let job = Job {
            numero: self.jobs.iter().map(|j| j.numero).max().unwrap_or(0) + 1,
            pgid: if self.interativo { pgid } else { pids[0] },
            pids,
            texto: texto_do_pipeline(&p),
            estado: EstadoJob::Rodando,
        };
        if p.fundo {
            println!("[{}] {}", job.numero, job.pids.last().unwrap_or(&0));
            self.jobs.push(job);
            return;
        }
        self.primeiro_plano(job);
    }

    /// Dá o terminal ao job e espera ele terminar ou parar (Ctrl+Z).
    fn primeiro_plano(&mut self, mut job: Job) {
        if self.interativo {
            let _ = sys::dar_terminal(0, job.pgid);
        }
        let ultimo = *job.pids.last().unwrap_or(&0);
        let opcoes = if self.interativo { sys::WUNTRACED } else { 0 };
        while let Some(&pid) = job.pids.first() {
            let status = match sys::esperar_com(pid, opcoes) {
                Ok(Some((_, status))) => status,
                _ => {
                    job.pids.remove(0);
                    continue;
                }
            };
            if let Some(sinal) = sys::sinal_que_parou(status) {
                job.estado = EstadoJob::Parado;
                println!("[{}]+  {:<10} {}", job.numero, "Parado", job.texto);
                self.ultimo_status = 128 + sinal;
                self.jobs.push(job);
                break;
            }
            job.pids.remove(0);
            if pid == ultimo {
                self.ultimo_status = status_para_codigo(status);
            }
            if let Some(sinal) = sys::sinal_que_matou(status) {
                // Ctrl+C e pipe quebrado são normais; o resto merece aviso.
                if sinal != sys::SIGINT && sinal != 13 {
                    eprintln!("{}: {} (sinal {})", job.texto, sys::nome_do_sinal(sinal), sinal);
                }
            }
        }
        if self.interativo {
            let _ = sys::dar_terminal(0, self.meu_grupo); // o terminal volta para o shell
        }
    }

    /// O job escolhido por `%N` (ou o mais recente). Sai da lista.
    fn tirar_job(&mut self, escolha: Option<&String>) -> Option<Job> {
        let i = match escolha {
            None => self.jobs.len().checked_sub(1)?,
            Some(e) => {
                let numero: usize = e.trim_start_matches('%').parse().ok()?;
                self.jobs.iter().position(|j| j.numero == numero)?
            }
        };
        Some(self.jobs.remove(i))
    }

    /// `jobs`, `fg` e `bg`.
    fn controle_de_jobs(&mut self, comando: &Comando) -> Option<i32> {
        let args = &comando.args;
        match args[0].as_str() {
            "jobs" => {
                for (i, j) in self.jobs.iter().enumerate() {
                    let marca = if i + 1 == self.jobs.len() { '+' } else { ' ' };
                    let estado = if j.estado == EstadoJob::Parado { "Parado" } else { "Rodando" };
                    println!("[{}]{}  {:<10} {}", j.numero, marca, estado, j.texto);
                }
                Some(0)
            }
            "fg" | "bg" => {
                let Some(mut job) = self.tirar_job(args.get(1)) else {
                    eprintln!("{}: job não existe", args[0]);
                    return Some(1);
                };
                let parado = job.estado == EstadoJob::Parado;
                job.estado = EstadoJob::Rodando;
                if args[0] == "bg" {
                    println!("[{}]+ {} &", job.numero, job.texto);
                    let _ = sys::matar(-job.pgid, sys::SIGCONT);
                    self.jobs.push(job);
                    return Some(0);
                }
                println!("{}", job.texto);
                if self.interativo {
                    let _ = sys::dar_terminal(0, job.pgid); // antes do SIGCONT: ele pode ler na hora
                }
                if parado {
                    let _ = sys::matar(-job.pgid, sys::SIGCONT);
                }
                self.primeiro_plano(job);
                Some(self.ultimo_status)
            }
            "exit" => self.sair(args.get(1).and_then(|c| c.parse().ok()).unwrap_or(0)),
            _ => None,
        }
    }

    /// Jobs parados ficariam parados para sempre depois que o shell sai: encerra.
    fn sair(&self, codigo: i32) -> ! {
        for job in self.jobs.iter().filter(|j| j.estado == EstadoJob::Parado) {
            let _ = sys::matar(-job.pgid, sys::SIGTERM);
            let _ = sys::matar(-job.pgid, sys::SIGCONT);
        }
        sys::sair(codigo)
    }

    /// `export NOME=valor` e `unset NOME`.
    fn variaveis(&mut self, comando: &Comando) -> Option<i32> {
        let args = &comando.args;
        match args[0].as_str() {
            "export" => {
                if args.len() == 1 {
                    for v in &self.ambiente {
                        println!("export {}", v);
                    }
                }
                for definicao in &args[1..] {
                    let Some((nome, _)) = definicao.split_once('=') else { continue };
                    let prefixo = format!("{}=", nome);
                    self.ambiente.retain(|v| !v.starts_with(&prefixo));
                    self.ambiente.push(definicao.clone());
                }
                Some(0)
            }
            "unset" => {
                for nome in &args[1..] {
                    let prefixo = format!("{}=", nome);
                    self.ambiente.retain(|v| !v.starts_with(&prefixo));
                }
                Some(0)
            }
            _ => None,
        }
    }

    /// Antes de cada prompt: quem, em segundo plano, terminou ou parou?
    fn recolher_segundo_plano(&mut self) {
        while let Ok(Some((pid, status))) = sys::esperar_com(-1, sys::WNOHANG | sys::WUNTRACED) {
            let Some(i) = self.jobs.iter().position(|j| j.pids.contains(&pid)) else { continue };
            if let Some(sinal) = sys::sinal_que_parou(status) {
                let job = &mut self.jobs[i];
                if job.estado != EstadoJob::Parado {
                    job.estado = EstadoJob::Parado;
                    println!("[{}]+  {} ({})", job.numero, job.texto, sys::nome_do_sinal(sinal));
                }
                continue;
            }
            let job = &mut self.jobs[i];
            job.pids.retain(|&p| p != pid);
            if job.pids.is_empty() {
                let como = match sys::sinal_que_matou(status) {
                    Some(sinal) => String::from(sys::nome_do_sinal(sinal)),
                    None => match status_para_codigo(status) {
                        0 => String::from("Concluído"),
                        c => format!("Saiu com {}", c),
                    },
                };
                println!("[{}]   {:<10} {}", job.numero, como, job.texto);
                self.jobs.remove(i);
            }
        }
    }
}

/// O que roda no processo filho: arruma os descritores e troca de programa.
/// `grupo`: com controle de jobs, o grupo onde entrar (0 = começar um novo).
fn filho(
    comando: &Comando,
    entrada: Option<i32>,
    pipe: Option<(i32, i32)>,
    fundo: bool,
    ambiente: &[String],
    grupo: Option<i32>,
) -> ! {
    match grupo {
        Some(pgid) => {
            let _ = sys::mudar_grupo(0, pgid);
            if !fundo {
                let _ = sys::dar_terminal(0, sys::grupo());
            }
            // O programa volta a reagir ao teclado normalmente.
            for sinal in [sys::SIGINT, sys::SIGTSTP, sys::SIGTTIN, sys::SIGTTOU] {
                sys::ignorar_sinal(sinal, false);
            }
        }
        None if !fundo => sys::ignorar_sinal(sys::SIGINT, false),
        None => {}
    }
    if let Some(fd) = entrada {
        let _ = sys::duplicar_para(fd, 0);
        let _ = sys::fechar(fd);
    }
    if let Some((leitura, escrita)) = pipe {
        let _ = sys::duplicar_para(escrita, 1);
        let _ = sys::fechar(escrita);
        let _ = sys::fechar(leitura);
    }
    if let Some(arquivo) = &comando.entrada {
        redirecionar(arquivo, sys::O_RDONLY, 0);
    }
    if let Some((arquivo, acrescentar)) = &comando.saida {
        let modo = if *acrescentar { sys::O_APPEND } else { sys::O_TRUNC };
        redirecionar(arquivo, sys::O_WRONLY | sys::O_CREAT | modo, 1);
    }

    if let Some(status) = interno(comando) {
        sys::sair(status);
    }

    let nome = &comando.args[0];
    let caminho = if nome.contains('/') { nome.clone() } else { format!("/bin/{}", nome) };
    let args: Vec<&str> = comando.args.iter().map(|s| s.as_str()).collect();
    let erro = sys::executar_com_ambiente(&caminho, &args, ambiente);
    if erro == sys::ENOENT {
        eprintln!("sh: {}: comando não encontrado (veja `ajuda`)", nome);
        sys::sair(127);
    }
    eprintln!("sh: {}: {}", nome, erro);
    sys::sair(126)
}

fn redirecionar(arquivo: &str, flags: u64, para: i32) {
    match sys::abrir(arquivo, flags) {
        Ok(fd) => {
            let _ = sys::duplicar_para(fd, para);
            let _ = sys::fechar(fd);
        }
        Err(e) => {
            eprintln!("sh: {}: {}", arquivo, e);
            sys::sair(1);
        }
    }
}

/// Comandos internos. `None` se não for um deles.
fn interno(comando: &Comando) -> Option<i32> {
    let args = &comando.args;
    match args[0].as_str() {
        "cd" => {
            let casa = casa();
            let destino = match args.get(1).map(|s| s.as_str()) {
                None | Some("~") => casa.as_str(),
                Some(d) => d,
            };
            Some(match sys::mudar_diretorio(destino) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("cd: {}: {}", destino, e);
                    1
                }
            })
        }
        "pwd" => {
            println!("{}", sys::diretorio_atual());
            Some(0)
        }
        "exit" => sys::sair(args.get(1).and_then(|c| c.parse().ok()).unwrap_or(0)),
        "ajuda" | "help" => {
            ajuda();
            Some(0)
        }
        _ => None,
    }
}

fn ajuda() {
    println!("\x1b[1mComandos internos do shell:\x1b[0m cd, pwd, export, unset, jobs, fg, bg, exit, ajuda");
    println!("\x1b[1mProgramas em /bin:\x1b[0m");
    if let Ok(mut programas) = sys::listar_diretorio("/bin") {
        programas.sort_by(|a, b| a.nome.cmp(&b.nome));
        let mut linha = String::from(" ");
        for p in programas {
            if linha.len() + p.nome.len() > 70 {
                println!("{}", linha);
                linha = String::from(" ");
            }
            linha.push(' ');
            linha.push_str(&p.nome);
        }
        println!("{}", linha);
    }
    println!("\x1b[1mOperadores:\x1b[0m  a | b   a > arq   a >> arq   a < arq   a &   a ; b   $?");
    println!("\x1b[1mTeclas:\x1b[0m  Ctrl+C interrompe   Ctrl+Z para (depois: fg ou bg)   Ctrl+D fim de entrada");
}
