//! Sinais: avisos assíncronos para um processo (Ctrl+C, `kill`, falhas, alarmes...).
//!
//! Para cada sinal, o processo escolhe uma ação: o padrão (quase sempre
//! morrer), ignorar, ou chamar uma função sua (o "tratador"). Também pode
//! *bloquear* sinais por um tempo: eles ficam pendentes até serem liberados.
//!
//! ## Como um tratador é chamado
//!
//! O kernel não pode simplesmente "chamar uma função" do programa: ele está em
//! outro anel, com outra pilha. O que ele faz, na volta para o modo usuário, é
//! montar um quadro na pilha do programa e trocar o RIP:
//!
//! ```text
//!   pilha do usuário (cresce para baixo)
//!   ┌──────────────────────────┐ ← RSP antigo
//!   │ 128 bytes intocados      │   (a "red zone" da ABI)
//!   │ máscara antiga           │
//!   │ Contexto salvo (176 B)   │   todos os registradores de antes
//!   │ siginfo (128 B)          │
//!   │ endereço de retorno ─────┼─► "restaurador" (código da libc: syscall rt_sigreturn)
//!   └──────────────────────────┘ ← RSP novo; RIP = tratador, RDI = número do sinal
//! ```
//!
//! Quando o tratador termina com `ret`, cai no restaurador, que chama
//! `rt_sigreturn`: o kernel lê o Contexto salvo e o programa continua
//! exatamente de onde estava. O formato é o mesmo do Linux (para a libc).

use crate::processo::Dividido;
use alloc::sync::Arc;

pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;
pub const SIGSEGV: u32 = 11;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
const SIGURG: u32 = 23;
const SIGWINCH: u32 = 28;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SA_RESTART: u64 = 0x1000_0000;
const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESETHAND: u64 = 0x8000_0000;

/// Sinais que ninguém pode bloquear nem tratar.
pub const INBLOQUEAVEIS: u64 = (1 << SIGKILL) | (1 << SIGSTOP);

/// Os que, por padrão, *param* o processo (até um SIGCONT).
pub const PARADAS: u64 = (1 << SIGSTOP) | (1 << SIGTSTP) | (1 << SIGTTIN) | (1 << SIGTTOU);

/// A `struct sigaction` do kernel Linux (32 bytes).
#[derive(Clone, Copy, Default)]
pub struct Acao {
    pub tratador: u64,
    pub flags: u64,
    pub restaurador: u64,
    pub mascara: u64,
}

impl Acao {
    pub fn tem_tratador(&self) -> bool {
        self.tratador > SIG_IGN
    }
}

/// O padrão destes é não fazer nada. (O SIGCONT faz seu trabalho — continuar
/// um processo parado — no momento em que é enviado, não quando é entregue.)
pub fn padrao_e_ignorar(sinal: u32) -> bool {
    matches!(sinal, SIGCHLD | SIGCONT | SIGURG | SIGWINCH)
}

pub fn padrao_e_parar(sinal: u32) -> bool {
    PARADAS & (1 << sinal) != 0
}

/// Pendentes e bloqueados são de cada thread; as ações (os tratadores) são
/// do processo todo, divididas entre as threads.
pub struct Sinais {
    pub pendentes: u64,
    pub bloqueados: u64,
    acoes: Arc<Dividido<[Acao; 64]>>,
}

impl Sinais {
    pub fn novo() -> Self {
        Self { pendentes: 0, bloqueados: 0, acoes: Dividido::novo([Acao::default(); 64]) }
    }

    pub fn acoes(&self) -> &mut [Acao; 64] {
        self.acoes.get()
    }

    /// Para um `fork`: as mesmas ações, mas uma cópia só do filho.
    pub fn duplicar(&self) -> Self {
        Self { pendentes: 0, bloqueados: self.bloqueados, acoes: Dividido::novo(*self.acoes.get()) }
    }

    /// Para uma thread nova: as ações são as mesmas (divididas).
    pub fn para_thread(&self) -> Self {
        Self { pendentes: 0, bloqueados: self.bloqueados, acoes: self.acoes.clone() }
    }

    fn ignorado(&self, sinal: u32) -> bool {
        if sinal == SIGKILL {
            return false;
        }
        match self.acoes()[sinal as usize].tratador {
            SIG_IGN => true,
            SIG_DFL => padrao_e_ignorar(sinal),
            _ => false,
        }
    }

    /// Marca o sinal como pendente. Devolve `false` se ele for ignorado
    /// (aí nem vale a pena acordar o processo).
    pub fn postar(&mut self, sinal: u32) -> bool {
        if self.ignorado(sinal) {
            return false;
        }
        self.pendentes |= 1 << sinal;
        true
    }

    /// Este sinal, agora, pararia o processo? (SIGSTOP sempre; os outros de
    /// parar, se estiverem com a ação padrão e não bloqueados.)
    pub fn pararia(&self, sinal: u32) -> bool {
        sinal == SIGSTOP
            || padrao_e_parar(sinal)
                && self.acoes()[sinal as usize].tratador == SIG_DFL
                && self.bloqueados & (1 << sinal) == 0
    }

    pub fn ignora(&self, sinal: u32) -> bool {
        self.ignorado(sinal) || self.bloqueados & (1 << sinal) != 0
    }

    /// Os que podem ser entregues agora (pendentes e não bloqueados).
    pub fn entregaveis(&self) -> u64 {
        self.pendentes & !(self.bloqueados & !INBLOQUEAVEIS)
    }

    /// O próximo a entregar, já tirado da lista de pendentes.
    pub fn retirar(&mut self) -> Option<u32> {
        let entregaveis = self.entregaveis();
        if entregaveis == 0 {
            return None;
        }
        let sinal = entregaveis.trailing_zeros();
        self.pendentes &= !(1 << sinal);
        Some(sinal)
    }

    /// Antes de chamar o tratador: bloqueia o próprio sinal (para ele não se
    /// interromper) e os da máscara pedida.
    pub fn ao_entregar(&mut self, sinal: u32) {
        let acao = self.acoes()[sinal as usize];
        if acao.flags & SA_NODEFER == 0 {
            self.bloqueados |= 1 << sinal;
        }
        self.bloqueados = (self.bloqueados | acao.mascara) & !INBLOQUEAVEIS;
        if acao.flags & SA_RESETHAND != 0 {
            self.acoes()[sinal as usize] = Acao::default();
        }
    }

    /// `execve` apaga o programa, e com ele os tratadores. Ignorados continuam ignorados.
    pub fn ao_executar(&mut self) {
        for acao in self.acoes().iter_mut() {
            if acao.tem_tratador() {
                *acao = Acao::default();
            }
        }
    }
}
