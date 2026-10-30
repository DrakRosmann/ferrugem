//! monitor — o monitor do sistema (rode dentro do `janelas`).
//!
//! Gráficos do uso de CPU e de memória no último minuto e a lista dos
//! processos (CPU, memória, usuário, estado), atualizados a cada segundo, e
//! botões para encerrar um processo. Tudo vem do `/proc`, como no `top` do
//! Linux: o `/proc/stat` diz quantos tiques do timer cada CPU passou ocupada
//! e ociosa; o uso de um processo é quanto os tiques dele aumentaram desde a
//! última olhada, em relação aos tiques que uma CPU teve nesse tempo.

#![no_std]
#![no_main]

use ulib::janela::Janela;
use ulib::sys;
use ulib::ui::{self, Dialogo, Icone, Lista, Ui};
use ulib::usuarios::nome_do_usuario;
use ulib::*;

programa!(principal);

const AMOSTRAS: usize = 60;

struct Processo {
    pid: i32,
    nome: String,
    estado: char,
    tiques: u64,
    paginas: u64,
    uid: u32,
    /// Uso de CPU desde a última olhada (em % de uma CPU).
    cpu: u32,
}

/// Lê /proc/<pid>/stat (o formato do Linux) e o Uid de /proc/<pid>/status.
fn ler_processo(pid: i32) -> Option<Processo> {
    let stat = String::from_utf8_lossy(&sys::ler_arquivo(&format!("/proc/{}/stat", pid)).ok()?).into_owned();
    let (inicio, fim) = (stat.find('(')?, stat.rfind(')')?);
    let nome = String::from(&stat[inicio + 1..fim]);
    let campos: Vec<&str> = stat[fim + 1..].split_whitespace().collect();
    let status = String::from_utf8_lossy(&sys::ler_arquivo(&format!("/proc/{}/status", pid)).ok()?).into_owned();
    let uid = status.lines().find(|l| l.starts_with("Uid:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(Processo {
        pid,
        nome,
        estado: campos.first()?.chars().next()?,
        tiques: campos.get(11)?.parse().ok()?,
        paginas: campos.get(21)?.parse().ok()?,
        uid,
        cpu: 0,
    })
}

/// (ocupado, ocioso) de cada CPU, em tiques, do /proc/stat.
fn tiques_das_cpus() -> Vec<(u64, u64)> {
    let texto = String::from_utf8_lossy(&sys::ler_arquivo("/proc/stat").unwrap_or_default()).into_owned();
    texto
        .lines()
        .filter(|l| l.starts_with("cpu") && !l.starts_with("cpu "))
        .filter_map(|l| {
            let c: Vec<u64> = l.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
            Some((*c.first()?, *c.get(3)?))
        })
        .collect()
}

fn memoria() -> (u64, u64) {
    let texto = String::from_utf8_lossy(&sys::ler_arquivo("/proc/meminfo").unwrap_or_default()).into_owned();
    let campo = |nome: &str| -> u64 {
        texto.lines().find(|l| l.starts_with(nome)).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0)
    };
    (campo("MemTotal:"), campo("MemFree:"))
}

/// Um gráfico de linha com a história (valores de 0 a 100).
fn grafico(t: &mut ui::Tela, titulo: &str, valores: &[u32], largura: i32, altura: i32) {
    let tema = t.tema();
    let (x, y) = t.reservar(largura, altura);
    t.q.retangulo(x, y, largura, altura, tema.borda);
    t.q.retangulo(x + 1, y + 1, largura - 2, altura - 2, tema.campo);
    for k in 1..4 {
        t.q.retangulo(x + 1, y + altura * k / 4, largura - 2, 1, tema.painel);
    }
    let ponto = |i: usize, v: u32| -> (i32, i32) {
        let px = x + 2 + (largura - 4) * (AMOSTRAS - valores.len() + i) as i32 / (AMOSTRAS as i32 - 1);
        (px, y + altura - 2 - (altura - 4) * v.min(100) as i32 / 100)
    };
    for i in 1..valores.len() {
        let ((x0, y0), (x1, y1)) = (ponto(i - 1, valores[i - 1]), ponto(i, valores[i]));
        t.q.linha(x0, y0, x1, y1, 2, tema.destaque);
    }
    let atual = valores.last().copied().unwrap_or(0);
    t.escrever(x + 8, y + 6, &format!("{}: {}%", titulo, atual), tema.texto, true, 13);
}

fn principal(_args: &[&str]) -> i32 {
    let mut janela = match Janela::abrir(640, 560, "Monitor do sistema") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("monitor: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let cpus = String::from_utf8_lossy(&sys::ler_arquivo("/proc/cpuinfo").unwrap_or_default()).matches("processor").count().max(1);
    let mut historico_cpu: Vec<u32> = Vec::new();
    let mut historico_memoria: Vec<u32> = Vec::new();
    let mut processos: Vec<Processo> = Vec::new();
    let mut anteriores: Vec<(i32, u64)> = Vec::new();
    let mut ultima = 0u64;
    let mut cpus_antes = tiques_das_cpus();
    let mut uso_por_cpu: Vec<u32> = Vec::new();
    let mut lista = Lista::default();
    let mut dialogo: Option<Dialogo> = None;
    let mut alvo: Option<(i32, i32)> = None; // (pid, sinal) a confirmar
    let mut mensagem = String::new();

    ui::laco_com_prazo(&mut janela, &mut ui, 1000, |t| {
        // Uma amostra nova a cada segundo (os outros desenhos só mostram).
        let agora = sys::tempo_ms();
        if agora - ultima >= 950 {
            // A primeira olhada só serve de ponto de partida (o intervalo seria quase zero).
            let primeira = ultima == 0;
            ultima = agora;
            // Quanto cada CPU trabalhou desde a última olhada.
            let cpus_agora = tiques_das_cpus();
            let deltas: Vec<(u64, u64)> = cpus_agora
                .iter()
                .zip(cpus_antes.iter().chain(core::iter::repeat(&(0, 0))))
                .map(|(a, b)| (a.0.saturating_sub(b.0), a.1.saturating_sub(b.1)))
                .collect();
            cpus_antes = cpus_agora;
            uso_por_cpu = deltas.iter().map(|(o, i)| if o + i > 0 { (o * 100 / (o + i)) as u32 } else { 0 }).collect();
            let (ocupado, todos) = deltas.iter().fold((0, 0), |(a, b), (o, i)| (a + o, b + o + i));
            // os tiques que uma CPU teve no intervalo
            let por_cpu = (todos / cpus as u64).max(1);
            let selecionado = lista.selecionado.and_then(|i| processos.get(i)).map(|p| p.pid);
            let mut novos: Vec<Processo> = sys::listar_diretorio("/proc")
                .unwrap_or_default()
                .iter()
                .filter_map(|e| e.nome.parse::<i32>().ok())
                .filter_map(ler_processo)
                .collect();
            for p in novos.iter_mut() {
                let antes = anteriores.iter().find(|(pid, _)| *pid == p.pid).map_or(p.tiques, |a| a.1);
                p.cpu = (p.tiques.saturating_sub(antes) * 100 / por_cpu).min(100) as u32;
            }
            anteriores = novos.iter().map(|p| (p.pid, p.tiques)).collect();
            let uso = if todos > 0 { (ocupado * 100 / todos) as u32 } else { 0 };
            if historico_cpu.len() == AMOSTRAS {
                historico_cpu.remove(0);
                historico_memoria.remove(0);
            }
            if !primeira {
                historico_cpu.push(uso);
                let (total_mem, livre) = memoria();
                historico_memoria.push(if total_mem > 0 { ((total_mem - livre) * 100 / total_mem) as u32 } else { 0 });
            }
            novos.sort_by(|a, b| b.cpu.cmp(&a.cpu).then(a.pid.cmp(&b.pid)));
            processos = novos;
            // o processo escolhido continua escolhido, mesmo mudando de lugar
            lista.selecionado = selecionado.and_then(|pid| processos.iter().position(|p| p.pid == pid));
        }

        let largura = t.largura_livre();
        t.linha(|t| {
            let w = (largura - 8) / 2;
            grafico(t, &format!("CPU ({} CPUs)", cpus), &historico_cpu, w, 110);
            grafico(t, "Memória", &historico_memoria, w, 110);
        });
        let por_cpu: Vec<String> = uso_por_cpu.iter().enumerate().map(|(i, u)| format!("CPU{} {}%", i, u)).collect();
        t.rotulo(&por_cpu.join("  ·  "));
        let (total_mem, livre) = memoria();
        t.rotulo(&format!(
            "{} processos · memória: {} de {} MiB em uso",
            processos.len(),
            (total_mem - livre) / 1024,
            total_mem / 1024
        ));
        let linhas: Vec<(Icone, Vec<String>)> = processos
            .iter()
            .map(|p| {
                let estado = match p.estado {
                    'R' => "rodando",
                    'S' => "dormindo",
                    'T' => "parado",
                    'Z' => "zumbi",
                    _ => "?",
                };
                (
                    Icone::Programa,
                    Vec::from([
                        format!("{}", p.pid),
                        p.nome.clone(),
                        nome_do_usuario(p.uid),
                        String::from(estado),
                        format!("{}%", p.cpu),
                        format!("{} KiB", p.paginas * 4),
                    ]),
                )
            })
            .collect();
        let colunas = [("PID", 56), ("Nome", largura - 446), ("Usuário", 90), ("Estado", 90), ("CPU", 60), ("Memória", 120)];
        let altura = t.altura_livre() - 34 - 34;
        t.tabela(&mut lista, &colunas, &linhas, largura, altura);
        let escolhido = lista.selecionado.and_then(|i| processos.get(i)).map(|p| (p.pid, p.nome.clone()));
        t.linha(|t| {
            if let Some((pid, nome)) = &escolhido {
                if t.botao("Encerrar") {
                    alvo = Some((*pid, sys::SIGTERM));
                    dialogo = Some(Dialogo::novo("Encerrar processo", &format!("Encerrar {} (pid {})?", nome, pid), &["Encerrar", "Cancelar"]));
                }
                if t.botao("Forçar (SIGKILL)") {
                    alvo = Some((*pid, sys::SIGKILL));
                    let texto = format!("Matar {} (pid {}) na hora? Ele não terá chance de salvar nada.", nome, pid);
                    dialogo = Some(Dialogo::novo("Forçar o fim", &texto, &["Matar", "Cancelar"]));
                }
            }
            t.rotulo(&mensagem);
        });
        if let Some(b) = t.dialogo(&mut dialogo) {
            if let (0, Some((pid, sinal))) = (b, alvo.take()) {
                mensagem = match sys::matar(pid, sinal) {
                    Ok(()) => format!("Sinal {} enviado ao pid {}", sinal, pid),
                    Err(e) => format!("Não deu: {}", e),
                };
            }
        }
    });
    0
}
