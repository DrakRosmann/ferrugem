//! sh: o shell do FerrugemOS.
//!
//! A linha passa por duas etapas: a análise léxica separa palavras e
//! operadores respeitando aspas, e o analisador monta listas (`;`, `&&`,
//! `||`, `&`) de pipelines (`|`) de comandos com redirecionamentos (`<`,
//! `>`, `>>`, `2>&1`). As palavras só são expandidas (`$VAR`, `$?`, `~`,
//! aspas) na hora de executar cada comando, como num shell POSIX.
//!
//! Cada pipeline vira um job: um grupo de processos que recebe o terminal
//! enquanto está em primeiro plano. Ctrl+Z para o job; `jobs`, `fg` e `bg`
//! o controlam.
//!
//! Uso: `sh` (interativo), `sh -c 'comando' [args]` ou `sh arquivo [args]`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ferrugem_user::sys::{self, Handler};
use ferrugem_user::{entry, eprintln, print, println, rt};

entry!(main);

/// Sinais de controle de jobs: o shell interativo os ignora (só um Ctrl+C
/// interrompe a leitura do prompt) e os filhos voltam ao padrão.
const JOB_SIGNALS: [u32; 4] = [sys::SIGQUIT, sys::SIGTSTP, sys::SIGTTIN, sys::SIGTTOU];
const BUILTINS: [&str; 12] =
    ["cd", "pwd", "exit", "export", "unset", "jobs", "fg", "bg", "wait", "kill", "ajuda", "help"];
const F_DUPFD_CLOEXEC: u64 = 1030;
/// Os de dois caracteres vêm antes, para `>>` não ser lido como `>` `>`.
const OPERATORS: [&str; 10] = ["||", "&&", ">>", ">&", "<&", "|", "&", ";", "<", ">"];

// --- Análise da linha -------------------------------------------------------

enum Token {
    /// Palavra crua, ainda com aspas e `$`.
    Word(String),
    /// `|`, `||`, `&`, `&&` ou `;`.
    Op(&'static str),
    /// Redirecionamento: o descritor, se escrito (o 2 de `2>`), e o operador.
    Redirect(Option<u64>, &'static str),
}

#[derive(Clone)]
struct Redirect {
    fd: u64,
    op: &'static str,
    target: String,
}

/// Um comando simples. Antes da expansão as palavras estão cruas.
#[derive(Clone, Default)]
struct Command {
    /// Atribuições `NOME=valor` antes do nome do programa.
    assignments: Vec<String>,
    argv: Vec<String>,
    redirects: Vec<Redirect>,
}

impl Command {
    fn describe(&self) -> String {
        let mut parts: Vec<String> = self.assignments.iter().chain(&self.argv).cloned().collect();
        for r in &self.redirects {
            let fd = if r.fd == default_fd(r.op) { String::new() } else { r.fd.to_string() };
            parts.push(format!("{fd}{}{}", r.op, r.target));
        }
        parts.join(" ")
    }
}

type Pipeline = Vec<Command>;

fn describe_pipeline(pipeline: &[Command]) -> String {
    pipeline.iter().map(Command::describe).collect::<Vec<_>>().join(" | ")
}

/// Pipelines ligados por `&&` e `||`, como `make && ./prog || echo falhou`.
struct List {
    /// Cada pipeline com o conector que o precede ("" no primeiro).
    pipelines: Vec<(&'static str, Pipeline)>,
    background: bool,
}

impl List {
    fn describe(&self) -> String {
        let mut text = String::new();
        for (connector, pipeline) in &self.pipelines {
            if !connector.is_empty() {
                text.push_str(&format!(" {connector} "));
            }
            text.push_str(&describe_pipeline(pipeline));
        }
        text
    }
}

fn default_fd(op: &str) -> u64 {
    if op.starts_with('<') { 0 } else { 1 }
}

fn operator_at(chars: &[char], i: usize) -> Option<&'static str> {
    OPERATORS.into_iter().find(|op| op.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c)))
}

/// Separa a linha em tokens. Espaços e operadores fora de aspas separam
/// palavras; `#` no início de uma palavra começa um comentário.
fn lex(line: &str) -> Result<Vec<Token>, &'static str> {
    let chars: Vec<char> = line.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    loop {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() || chars[i] == '#' {
            return Ok(tokens);
        }
        if let Some(op) = operator_at(&chars, i) {
            i += op.len();
            tokens.push(if op.starts_with(['<', '>']) { Token::Redirect(None, op) } else { Token::Op(op) });
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && operator_at(&chars, i).is_none() {
            match chars[i] {
                '\'' => {
                    let end = chars[i + 1..].iter().position(|&c| c == '\'').ok_or("aspas simples sem fim")?;
                    i += end + 2;
                }
                '"' => {
                    i += 1;
                    loop {
                        match chars.get(i) {
                            None => return Err("aspas duplas sem fim"),
                            Some('"') => break,
                            Some('\\') => i += 2,
                            Some(_) => i += 1,
                        }
                    }
                    i += 1;
                }
                '\\' => i += 2,
                _ => i += 1,
            }
        }
        i = i.min(chars.len());
        let word: String = chars[start..i].iter().collect();
        // Um número colado a `<` ou `>` é o descritor redirecionado.
        match operator_at(&chars, i) {
            Some(op) if op.starts_with(['<', '>']) && word.bytes().all(|b| b.is_ascii_digit()) => {
                i += op.len();
                tokens.push(Token::Redirect(word.parse().ok(), op));
            }
            _ => tokens.push(Token::Word(word)),
        }
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn parse(tokens: Vec<Token>) -> Result<Vec<List>, String> {
    let mut tokens = tokens.into_iter().peekable();
    let mut lists = Vec::new();
    while tokens.peek().is_some() {
        let mut list = List { pipelines: Vec::new(), background: false };
        let mut connector = "";
        loop {
            let mut pipeline = Vec::new();
            loop {
                pipeline.push(parse_command(&mut tokens)?);
                if tokens.next_if(|t| matches!(t, Token::Op("|"))).is_none() {
                    break;
                }
            }
            list.pipelines.push((connector, pipeline));
            match tokens.next() {
                Some(Token::Op(op @ ("&&" | "||"))) => connector = op,
                Some(Token::Op("&")) => {
                    list.background = true;
                    break;
                }
                _ => break, // `;` ou fim da linha
            }
        }
        lists.push(list);
    }
    Ok(lists)
}

fn parse_command(tokens: &mut core::iter::Peekable<alloc::vec::IntoIter<Token>>) -> Result<Command, String> {
    let mut command = Command::default();
    loop {
        match tokens.next_if(|t| !matches!(t, Token::Op(_))) {
            Some(Token::Word(word)) => {
                if command.argv.is_empty() && is_assignment(&word) {
                    command.assignments.push(word);
                } else {
                    command.argv.push(word);
                }
            }
            Some(Token::Redirect(fd, op)) => match tokens.next() {
                Some(Token::Word(target)) => {
                    command.redirects.push(Redirect { fd: fd.unwrap_or(default_fd(op)), op, target })
                }
                _ => return Err(format!("erro de sintaxe: falta o arquivo depois de `{op}`")),
            },
            _ => break,
        }
    }
    if command.argv.is_empty() && command.assignments.is_empty() && command.redirects.is_empty() {
        return Err(match tokens.peek() {
            Some(Token::Op(op)) => format!("erro de sintaxe perto de `{op}`"),
            _ => String::from("erro de sintaxe: fim inesperado da linha"),
        });
    }
    Ok(command)
}

/// Aplica um redirecionamento ao próprio processo.
fn apply_redirect(r: &Redirect) -> Result<(), String> {
    let opened = match r.op {
        ">&" | "<&" => {
            if r.target == "-" {
                let _ = sys::close(r.fd);
                return Ok(());
            }
            let source: u64 = r.target.parse().map_err(|_| format!("{}: descritor inválido", r.target))?;
            sys::dup2(source, r.fd).map_err(|errno| format!("{source}: {}", sys::strerror(errno)))?;
            return Ok(());
        }
        "<" => sys::open(&r.target, sys::O_RDONLY, 0),
        ">" => sys::open(&r.target, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644),
        _ => sys::open(&r.target, sys::O_WRONLY | sys::O_CREAT | sys::O_APPEND, 0o644),
    };
    let fd = opened.map_err(|errno| format!("{}: {}", r.target, sys::strerror(errno)))?;
    if fd != r.fd {
        let _ = sys::dup2(fd, r.fd);
        let _ = sys::close(fd);
    }
    Ok(())
}

fn read_file(path: &str) -> sys::Result<String> {
    let fd = sys::open(path, sys::O_RDONLY, 0)?;
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sys::read(fd, &mut buf);
        if n <= 0 {
            let _ = sys::close(fd);
            return if n < 0 { Err(-n as sys::Errno) } else { Ok(String::from_utf8_lossy(&data).into_owned()) };
        }
        data.extend_from_slice(&buf[..n as usize]);
    }
}

// --- Jobs ---------------------------------------------------------------------

struct Job {
    id: usize,
    pgid: u64,
    /// Processos que ainda não terminaram.
    running: Vec<u64>,
    /// O último processo do pipeline: o estado dele é o do job.
    last: u64,
    /// Estado (no formato de wait4) do último processo, quando terminar.
    status: u32,
    /// Sinal que parou o job.
    stopped: Option<u32>,
    /// Terminou ou parou e o usuário ainda não foi avisado.
    changed: bool,
    text: String,
}

impl Job {
    fn state(&self) -> String {
        if self.running.is_empty() {
            match (sys::exit_code(self.status), sys::term_signal(self.status)) {
                (Some(0), _) => String::from("Concluído"),
                (Some(code), _) => format!("Saiu {code}"),
                (_, Some(signal)) => String::from(sys::signal_description(signal)),
                _ => String::from("?"),
            }
        } else if self.stopped.is_some() {
            String::from("Parado")
        } else {
            String::from("Executando")
        }
    }
}

/// Converte o estado de wait4 em código de saída (`$?`).
fn exit_status(status: u32) -> i32 {
    match (sys::exit_code(status), sys::term_signal(status)) {
        (Some(code), _) => code,
        (_, Some(signal)) => 128 + signal as i32,
        _ => 0,
    }
}

// --- O shell --------------------------------------------------------------------

struct Shell {
    /// Variáveis de ambiente, no formato "NOME=valor".
    env: Vec<String>,
    /// `$0`, `$1`...
    args: Vec<String>,
    /// Código de saída do último comando (`$?`).
    status: i32,
    /// Interativo: mostra o prompt e faz controle de jobs.
    interactive: bool,
    pid: u64,
    /// Grupo do próprio shell, que recebe o terminal de volta.
    pgid: u64,
    jobs: Vec<Job>,
    last_background: Option<u64>,
}

impl Shell {
    fn new() -> Self {
        let mut env: Vec<String> = rt::env().map(String::from).collect();
        if !env.iter().any(|e| e.starts_with("PATH=")) {
            env.push(String::from("PATH=/bin"));
        }
        Shell {
            env,
            args: Vec::from([String::from("sh")]),
            status: 0,
            interactive: false,
            pid: sys::getpid(),
            pgid: 0,
            jobs: Vec::new(),
            last_background: None,
        }
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.env.iter().find_map(|e| e.strip_prefix(name)?.strip_prefix('='))
    }

    fn set_var(&mut self, name: &str, value: &str) {
        self.unset_var(name);
        self.env.push(format!("{name}={value}"));
    }

    fn unset_var(&mut self, name: &str) {
        self.env.retain(|e| e.split_once('=').map(|(n, _)| n) != Some(name));
    }

    // --- Expansão -------------------------------------------------------------

    /// Valor de `$...`, lendo o nome a partir de `chars[*i]`.
    fn parameter(&self, chars: &[char], i: &mut usize) -> String {
        match chars.get(*i) {
            Some(&c @ ('?' | '$' | '!' | '#' | '@' | '0'..='9')) => {
                *i += 1;
                match c {
                    '?' => self.status.to_string(),
                    '$' => self.pid.to_string(),
                    '!' => self.last_background.map(|pid| pid.to_string()).unwrap_or_default(),
                    '#' => (self.args.len() - 1).to_string(),
                    '@' => self.args[1..].join(" "),
                    digit => self.args.get(digit as usize - '0' as usize).cloned().unwrap_or_default(),
                }
            }
            Some('{') => {
                let start = *i + 1;
                let end = chars[start..].iter().position(|&c| c == '}').map_or(chars.len(), |p| start + p);
                let name: String = chars[start..end].iter().collect();
                *i = (end + 1).min(chars.len());
                self.var(&name).unwrap_or("").to_string()
            }
            _ => {
                let start = *i;
                while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
                    *i += 1;
                }
                if *i == start {
                    return String::from("$");
                }
                let name: String = chars[start..*i].iter().collect();
                self.var(&name).unwrap_or("").to_string()
            }
        }
    }

    /// Tira as aspas e expande `~` e `$`. Uma palavra sem aspas que fica
    /// vazia (como `$NADA`) desaparece.
    fn expand_word(&self, raw: &str) -> Option<String> {
        let chars: Vec<char> = raw.chars().collect();
        let mut word = String::new();
        let mut quoted = false;
        let mut i = 0;
        if chars.first() == Some(&'~') && chars.get(1).is_none_or(|&c| c == '/') {
            word.push_str(self.var("HOME").unwrap_or("/"));
            i = 1;
        }
        while i < chars.len() {
            match chars[i] {
                '\'' => {
                    quoted = true;
                    let end = chars[i + 1..].iter().position(|&c| c == '\'').map_or(chars.len(), |p| i + 1 + p);
                    word.extend(&chars[i + 1..end]);
                    i = end + 1;
                }
                '"' => {
                    quoted = true;
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        match chars[i] {
                            '\\' if matches!(chars.get(i + 1), Some('"' | '\\' | '$')) => {
                                word.push(chars[i + 1]);
                                i += 2;
                            }
                            '$' => {
                                i += 1;
                                word.push_str(&self.parameter(&chars, &mut i));
                            }
                            c => {
                                word.push(c);
                                i += 1;
                            }
                        }
                    }
                    i += 1;
                }
                '\\' if i + 1 < chars.len() => {
                    word.push(chars[i + 1]);
                    i += 2;
                }
                '$' => {
                    i += 1;
                    word.push_str(&self.parameter(&chars, &mut i));
                }
                c => {
                    word.push(c);
                    i += 1;
                }
            }
        }
        (quoted || !word.is_empty()).then_some(word)
    }

    fn expand_command(&self, command: &Command) -> Command {
        let assignments = command
            .assignments
            .iter()
            .map(|a| {
                let (name, value) = a.split_once('=').unwrap();
                format!("{name}={}", self.expand_word(value).unwrap_or_default())
            })
            .collect();
        let argv = command.argv.iter().filter_map(|w| self.expand_word(w)).collect();
        let redirects = command
            .redirects
            .iter()
            .map(|r| Redirect { target: self.expand_word(&r.target).unwrap_or_default(), ..r.clone() })
            .collect();
        Command { assignments, argv, redirects }
    }

    /// Caminho do programa: o próprio nome se tiver '/', senão a primeira
    /// ocorrência nos diretórios do PATH.
    fn find_program(&self, name: &str) -> Option<String> {
        if name.contains('/') {
            return Some(String::from(name));
        }
        self.var("PATH")?
            .split(':')
            .map(|dir| format!("{}/{name}", dir.trim_end_matches('/')))
            .find(|path| sys::stat(path).is_ok_and(|info| !info.is_dir()))
    }

    // --- Execução -------------------------------------------------------------

    fn run_line(&mut self, line: &str) {
        let lists = match lex(line).map_err(String::from).and_then(parse) {
            Ok(lists) => lists,
            Err(message) => {
                eprintln!("sh: {message}");
                self.status = 2;
                return;
            }
        };
        for list in &lists {
            self.run_list(list);
        }
    }

    fn run_list(&mut self, list: &List) {
        if !list.background {
            self.run_and_or(&list.pipelines);
            return;
        }
        // Em segundo plano, um pipeline sozinho é o próprio job; uma lista
        // com `&&`/`||` roda inteira num subshell, que é o job.
        let job = match list.pipelines.as_slice() {
            [(_, pipeline)] => {
                let commands: Vec<Command> = pipeline.iter().map(|c| self.expand_command(c)).collect();
                self.launch(commands.len(), true, describe_pipeline(pipeline), |shell, i| shell.exec(&commands[i]))
            }
            pipelines => self.launch(1, true, list.describe(), |shell, _| {
                shell.run_and_or(pipelines);
                shell.status
            }),
        };
        if let Some(index) = job {
            let job = &self.jobs[index];
            if self.interactive {
                println!("[{}] {}", job.id, job.last);
            }
            self.last_background = Some(job.last);
            self.status = 0;
        }
    }

    fn run_and_or(&mut self, pipelines: &[(&'static str, Pipeline)]) {
        for (connector, pipeline) in pipelines {
            let skip = match *connector {
                "&&" => self.status != 0,
                "||" => self.status == 0,
                _ => false,
            };
            if !skip {
                self.status = self.run_pipeline(pipeline);
            }
        }
    }

    /// Executa um pipeline em primeiro plano e devolve o código de saída.
    fn run_pipeline(&mut self, pipeline: &[Command]) -> i32 {
        let commands: Vec<Command> = pipeline.iter().map(|c| self.expand_command(c)).collect();
        // Comandos internos sozinhos rodam no próprio shell (o `cd` precisa
        // mudar o diretório *deste* processo); num pipeline, num filho.
        if let [command] = commands.as_slice()
            && command.argv.first().is_none_or(|name| BUILTINS.contains(&name.as_str()))
        {
            return self.run_here(command);
        }
        match self.launch(commands.len(), false, describe_pipeline(pipeline), |shell, i| shell.exec(&commands[i])) {
            Some(index) => self.wait_foreground(index),
            None => 1,
        }
    }

    /// Cria os processos de um job, ligados por pipes e num grupo só, e
    /// devolve o índice dele em `jobs`. `body(i)` roda no i-ésimo filho e
    /// devolve o código de saída dele.
    fn launch(
        &mut self,
        count: usize,
        background: bool,
        text: String,
        body: impl Fn(&mut Shell, usize) -> i32,
    ) -> Option<usize> {
        let mut pgid = 0;
        let mut pids = Vec::new();
        let mut input: Option<u64> = None;
        for i in 0..count {
            let pipe = if i + 1 < count {
                match sys::pipe() {
                    Ok(pipe) => Some(pipe),
                    Err(errno) => {
                        eprintln!("sh: pipe: {}", sys::strerror(errno));
                        break;
                    }
                }
            } else {
                None
            };
            let forked = sys::fork();
            match forked {
                Ok(0) => {
                    self.enter_child(pgid, background);
                    if let Some(fd) = input {
                        let _ = sys::dup2(fd, 0);
                        let _ = sys::close(fd);
                    }
                    if let Some((read, write)) = pipe {
                        let _ = sys::close(read);
                        let _ = sys::dup2(write, 1);
                        let _ = sys::close(write);
                    }
                    sys::exit(body(self, i));
                }
                Ok(pid) => {
                    if pgid == 0 {
                        pgid = pid;
                    }
                    // O filho faz o mesmo: quem chegar antes evita a corrida.
                    if self.interactive {
                        let _ = sys::setpgid(pid, pgid);
                    }
                    pids.push(pid);
                }
                Err(errno) => eprintln!("sh: fork: {}", sys::strerror(errno)),
            }
            if let Some(fd) = input.take() {
                let _ = sys::close(fd);
            }
            if let Some((read, write)) = pipe {
                let _ = sys::close(write);
                input = Some(read);
            }
            if forked.is_err() {
                break;
            }
        }
        if let Some(fd) = input {
            let _ = sys::close(fd);
        }
        let last = *pids.last()?;
        let id = self.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        self.jobs.push(Job { id, pgid, running: pids, last, status: 0, stopped: None, changed: false, text });
        Some(self.jobs.len() - 1)
    }

    /// Prepara um filho recém-criado: grupo, terminal e sinais. O filho é
    /// um subshell não interativo, sem os jobs do pai.
    fn enter_child(&mut self, pgid: u64, background: bool) {
        if self.interactive {
            let pgid = if pgid == 0 { sys::getpid() } else { pgid };
            let _ = sys::setpgid(0, pgid);
            if !background {
                let _ = sys::tcsetpgrp(0, pgid);
            }
            for signal in [sys::SIGINT].into_iter().chain(JOB_SIGNALS) {
                let _ = sys::sigaction(signal, Handler::Default, 0);
            }
        } else if background {
            // Sem controle de jobs, o Ctrl+C não deve chegar ao segundo plano.
            let _ = sys::sigaction(sys::SIGINT, Handler::Ignore, 0);
            let _ = sys::sigaction(sys::SIGQUIT, Handler::Ignore, 0);
        }
        self.interactive = false;
        self.jobs.clear();
    }

    /// No filho: aplica os redirecionamentos e troca de programa.
    fn exec(&mut self, command: &Command) -> i32 {
        for redirect in &command.redirects {
            if let Err(message) = apply_redirect(redirect) {
                eprintln!("sh: {message}");
                return 1;
            }
        }
        for assignment in &command.assignments {
            let (name, value) = assignment.split_once('=').unwrap();
            self.set_var(name, value);
        }
        let Some(name) = command.argv.first() else { return 0 };
        if let Some(code) = self.builtin(&command.argv) {
            return code;
        }
        let Some(path) = self.find_program(name) else {
            eprintln!("sh: {name}: comando não encontrado");
            return 127;
        };
        let args: Vec<&str> = command.argv.iter().map(String::as_str).collect();
        let env: Vec<&str> = self.env.iter().map(String::as_str).collect();
        let errno = sys::execve(&path, &args, &env);
        eprintln!("sh: {name}: {}", sys::strerror(errno));
        if errno == sys::ENOENT { 127 } else { 126 }
    }

    /// Roda um comando interno (ou só atribuições/redirecionamentos) no
    /// próprio shell, desfazendo os redirecionamentos no fim.
    fn run_here(&mut self, command: &Command) -> i32 {
        let mut saved = Vec::new();
        let mut code = 0;
        for redirect in &command.redirects {
            saved.push((redirect.fd, sys::fcntl(redirect.fd, F_DUPFD_CLOEXEC, 10).ok()));
            if let Err(message) = apply_redirect(redirect) {
                eprintln!("sh: {message}");
                code = 1;
                break;
            }
        }
        if code == 0 {
            if command.argv.is_empty() {
                for assignment in &command.assignments {
                    let (name, value) = assignment.split_once('=').unwrap();
                    self.set_var(name, value);
                }
            } else {
                code = self.builtin(&command.argv).unwrap_or(127);
            }
        }
        for (fd, copy) in saved.into_iter().rev() {
            match copy {
                Some(copy) => {
                    let _ = sys::dup2(copy, fd);
                    let _ = sys::close(copy);
                }
                None => {
                    let _ = sys::close(fd);
                }
            }
        }
        code
    }

    // --- Espera e controle de jobs ---------------------------------------------

    /// Registra uma mudança de estado informada por wait4.
    fn record(&mut self, pid: u64, status: u32) {
        let Some(job) = self.jobs.iter_mut().find(|j| j.running.contains(&pid)) else { return };
        if let Some(signal) = sys::stop_signal(status) {
            if job.stopped.is_none() {
                job.stopped = Some(signal);
                job.changed = true;
            }
            return;
        }
        job.running.retain(|&p| p != pid);
        if pid == job.last {
            job.status = status;
        }
        if job.running.is_empty() {
            job.changed = true;
        }
    }

    /// Recolhe, sem bloquear, os filhos que terminaram ou pararam.
    fn collect(&mut self) {
        while let Ok((pid, status)) = sys::waitpid(-1, sys::WNOHANG | sys::WUNTRACED) {
            if pid == 0 {
                break;
            }
            self.record(pid, status);
        }
    }

    /// Mostra os jobs que mudaram (ou todos) e esquece os que terminaram.
    fn report(&mut self, all: bool) {
        let show = self.interactive || all;
        for job in &mut self.jobs {
            if show && (all || job.changed) {
                println!("[{}]  {:<12}{}", job.id, job.state(), job.text);
            }
            job.changed = false;
        }
        self.jobs.retain(|job| !job.running.is_empty());
    }

    /// Dá o terminal ao job e espera ele terminar ou parar.
    fn wait_foreground(&mut self, index: usize) -> i32 {
        while !self.jobs[index].running.is_empty() && self.jobs[index].stopped.is_none() {
            match sys::waitpid(-1, sys::WUNTRACED) {
                Ok((pid, status)) => self.record(pid, status),
                Err(sys::EINTR) => continue,
                Err(_) => break,
            }
        }
        if self.interactive {
            let _ = sys::tcsetpgrp(0, self.pgid);
        }
        let job = &mut self.jobs[index];
        if let Some(signal) = job.stopped {
            job.changed = false;
            println!("[{}]  {:<12}{}", job.id, "Parado", job.text);
            return 128 + signal as i32;
        }
        let job = self.jobs.remove(index);
        if let Some(signal) = sys::term_signal(job.status)
            && signal != sys::SIGINT
            && signal != sys::SIGPIPE
        {
            eprintln!("{}", sys::signal_description(signal));
        }
        exit_status(job.status)
    }

    /// `%n` ou `n`: o job de número n; sem argumento, o mais recente.
    fn find_job(&self, arg: Option<&str>) -> Option<usize> {
        match arg {
            None => self.jobs.len().checked_sub(1),
            Some(arg) => {
                let id: usize = arg.trim_start_matches('%').parse().ok()?;
                self.jobs.iter().position(|j| j.id == id)
            }
        }
    }

    /// `fg` e `bg`: continua um job parado, em primeiro ou segundo plano.
    fn resume(&mut self, args: &[&str]) -> i32 {
        let foreground = args[0] == "fg";
        if !self.interactive {
            eprintln!("{}: sem controle de jobs", args[0]);
            return 1;
        }
        self.collect();
        let Some(index) = self.find_job(args.get(1).copied()) else {
            eprintln!("{}: não existe esse job", args[0]);
            return 1;
        };
        let job = &mut self.jobs[index];
        job.stopped = None;
        job.changed = false;
        if foreground {
            println!("{}", job.text);
            let _ = sys::tcsetpgrp(0, job.pgid);
        } else {
            println!("[{}] {} &", job.id, job.text);
        }
        let _ = sys::kill(-(job.pgid as i64), sys::SIGCONT);
        if foreground {
            return self.wait_foreground(index);
        }
        self.last_background = Some(job.last);
        0
    }

    /// `kill [-SINAL] alvo...`: como o /bin/kill, mas o alvo também pode
    /// ser um job (`%n`), que recebe o sinal no grupo inteiro.
    fn kill(&mut self, args: &[&str]) -> i32 {
        let mut signal = sys::SIGTERM;
        let mut targets = &args[1..];
        if let Some(name) = targets.first().and_then(|a| a.strip_prefix('-'))
            && let Some(parsed) = sys::parse_signal(name)
        {
            signal = parsed;
            targets = &targets[1..];
        }
        if targets.is_empty() {
            eprintln!("uso: kill [-SINAL] pid|%job...");
            return 2;
        }
        self.collect();
        let mut status = 0;
        for target in targets {
            let (pid, stopped) = if target.starts_with('%') {
                match self.find_job(Some(target)) {
                    Some(index) => (-(self.jobs[index].pgid as i64), self.jobs[index].stopped.is_some()),
                    None => {
                        eprintln!("kill: {target}: não existe esse job");
                        status = 1;
                        continue;
                    }
                }
            } else {
                match target.parse::<i64>() {
                    Ok(pid) => (pid, false),
                    Err(_) => {
                        eprintln!("kill: pid inválido: {target}");
                        status = 1;
                        continue;
                    }
                }
            };
            if let Err(errno) = sys::kill(pid, signal) {
                eprintln!("kill: {target}: {}", sys::strerror(errno));
                status = 1;
            } else if stopped && signal != sys::SIGCONT {
                // Um job parado só recebe o sinal quando volta a rodar.
                let _ = sys::kill(pid, sys::SIGCONT);
            }
        }
        status
    }

    /// `wait`: espera todos os jobs em execução.
    fn wait_all(&mut self) -> i32 {
        while self.jobs.iter().any(|j| !j.running.is_empty() && j.stopped.is_none()) {
            match sys::waitpid(-1, sys::WUNTRACED) {
                Ok((pid, status)) => self.record(pid, status),
                Err(sys::EINTR) => return 128 + sys::SIGINT as i32,
                Err(_) => break,
            }
        }
        0
    }

    /// Sai do shell. Jobs parados levam SIGHUP (e SIGCONT, para recebê-lo):
    /// sem o shell, ninguém mais poderia continuá-los.
    fn leave(&mut self, code: i32) -> ! {
        for job in self.jobs.iter().filter(|j| j.stopped.is_some()) {
            let _ = sys::kill(-(job.pgid as i64), sys::SIGHUP);
            let _ = sys::kill(-(job.pgid as i64), sys::SIGCONT);
        }
        sys::exit(code)
    }

    /// Comandos internos. Devolve `None` se `argv` não for um deles.
    fn builtin(&mut self, argv: &[String]) -> Option<i32> {
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let code = match args[0] {
            "cd" => {
                let target = args.get(1).copied().or(self.var("HOME")).unwrap_or("/").to_string();
                match sys::chdir(&target) {
                    Ok(()) => {
                        let cwd = sys::getcwd().unwrap_or(target);
                        self.set_var("PWD", &cwd);
                        0
                    }
                    Err(errno) => {
                        eprintln!("cd: {target}: {}", sys::strerror(errno));
                        1
                    }
                }
            }
            "pwd" => match sys::getcwd() {
                Ok(cwd) => {
                    println!("{cwd}");
                    0
                }
                Err(_) => 1,
            },
            "exit" => {
                let code = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(self.status);
                self.leave(code)
            }
            "export" => {
                for assignment in &args[1..] {
                    if let Some((name, value)) = assignment.split_once('=') {
                        self.set_var(name, value);
                    }
                }
                0
            }
            "unset" => {
                for name in &args[1..] {
                    self.unset_var(name);
                }
                0
            }
            "jobs" => {
                self.collect();
                self.report(true);
                0
            }
            "fg" | "bg" => self.resume(&args),
            "wait" => self.wait_all(),
            "kill" => self.kill(&args),
            "ajuda" | "help" => {
                help();
                0
            }
            _ => return None,
        };
        Some(code)
    }

    // --- Interação --------------------------------------------------------------

    /// Assume o terminal: espera estar em primeiro plano (pode ter sido
    /// iniciado com `sh &`), ignora os sinais de jobs e cria o próprio grupo.
    fn take_terminal(&mut self) {
        while let (Ok(foreground), Ok(group)) = (sys::tcgetpgrp(0), sys::getpgid(0)) {
            if foreground == group {
                break;
            }
            let _ = sys::kill(0, sys::SIGTTIN);
        }
        let _ = sys::sigaction(sys::SIGINT, Handler::Function(on_interrupt), 0);
        for signal in JOB_SIGNALS {
            let _ = sys::sigaction(signal, Handler::Ignore, 0);
        }
        let _ = sys::setpgid(0, 0); // falha (sem problema) se já for líder de sessão
        self.pgid = sys::getpgid(0).unwrap_or(self.pid);
        let _ = sys::tcsetpgrp(0, self.pgid);
    }

    fn prompt(&self) {
        let cwd = sys::getcwd().unwrap_or_default();
        let home = self.var("HOME").unwrap_or("");
        let shown = match cwd.strip_prefix(home) {
            Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
            _ => cwd.clone(),
        };
        print!("\x1b[1;32mferrugem\x1b[0m:\x1b[1;34m{shown}\x1b[0m$ ");
    }

    /// Lê e executa comandos da entrada padrão até o fim.
    fn run_stdin(&mut self) -> ! {
        let mut pending = String::new();
        let mut buf = [0u8; 1024];
        loop {
            self.collect();
            self.report(false);
            // Só no começo de uma linha: uma linha longa chega em vários `read`.
            if self.interactive && pending.is_empty() {
                self.prompt();
            }
            if let Some(end) = pending.find('\n') {
                let line: String = pending.drain(..=end).collect();
                self.run_line(line.trim_end_matches('\n'));
                continue;
            }
            let n = sys::read(0, &mut buf);
            if n == -(sys::EINTR as i64) {
                pending.clear(); // Ctrl+C: o terminal já mostrou ^C
                continue;
            }
            if n <= 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
        }
        if !pending.is_empty() {
            let line = core::mem::take(&mut pending);
            self.run_line(&line);
        }
        if self.interactive {
            println!("exit");
        }
        let status = self.status;
        self.leave(status)
    }
}

/// O Ctrl+C no prompt só interrompe a leitura (para mostrar um prompt novo).
extern "C" fn on_interrupt(_: i32) {}

fn help() {
    const LINES: &[(&str, &str)] = &[
        ("a | b", "a saída de a vira a entrada de b"),
        ("< arq  > arq  >> arq", "redireciona a entrada / saída (2> para erros)"),
        ("2>&1", "erros vão para onde vai a saída"),
        ("a ; b   a && b   a || b", "em sequência / se a deu certo / se a falhou"),
        ("cmd &", "roda em segundo plano"),
        ("jobs  fg [%n]  bg [%n]", "controle de jobs (Ctrl+Z para o atual)"),
        ("wait  kill %n", "espera os jobs / envia um sinal a um job"),
        ("cd  pwd  export  unset", "diretório e variáveis"),
        ("$VAR ${VAR} $? $$ $! ~", "expansões (também $0..$9, $#, $@)"),
        ("exit [n]", "sai do shell (Ctrl+D também)"),
    ];
    for (syntax, description) in LINES {
        println!("  \x1b[1;36m{syntax:<26}\x1b[0m{description}");
    }
    println!("Os programas ficam em /bin: experimente 'ls /bin'.");
}

fn main() -> i32 {
    let args: Vec<&str> = rt::args().collect();
    let mut shell = Shell::new();
    match args.get(1).copied() {
        Some("-c") => {
            let Some(command) = args.get(2) else {
                eprintln!("sh: -c: falta o comando");
                return 2;
            };
            if args.len() > 3 {
                shell.args = args[3..].iter().map(|a| String::from(*a)).collect();
            }
            shell.run_line(command);
            shell.status
        }
        Some(path) => {
            let script = match read_file(path) {
                Ok(script) => script,
                Err(errno) => {
                    eprintln!("sh: {path}: {}", sys::strerror(errno));
                    return 127;
                }
            };
            shell.args = args[1..].iter().map(|a| String::from(*a)).collect();
            for line in script.lines() {
                shell.collect();
                shell.report(false);
                shell.run_line(line);
            }
            shell.status
        }
        None => {
            shell.interactive = sys::isatty(0);
            if shell.interactive {
                shell.take_terminal();
                println!("sh do FerrugemOS. 'ajuda' mostra a sintaxe; 'exit' (ou Ctrl+D) sai.");
            }
            shell.run_stdin()
        }
    }
}
