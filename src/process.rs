//! Processos de usuário: carregamento de executáveis, regiões de memória,
//! paginação sob demanda, `fork` com cópia sob escrita, `execve` e `exit`.
//!
//! Cada processo descreve sua memória como uma lista de regiões (`Vma`):
//! a imagem do ELF, o heap (`brk`), a pilha e as áreas de `mmap`. Só a imagem
//! e o topo da pilha são mapeados de saída; o resto ganha páginas na primeira
//! vez que é tocado, pelo tratador de falha de página.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::cpu::{self, FpuState};
use crate::elf::{self, PF_R, PF_W, PF_X};
use crate::file::FdTable;
use crate::gdt;
use crate::interrupts::InterruptFrame;
use crate::memory::FRAME_SIZE;
use crate::paging::{AddressSpace, COPY_ON_WRITE, NO_EXECUTE, USER_TOP, WRITABLE};
use crate::signal::SignalState;
use crate::syscall::{E2BIG, EACCES, EFAULT, EINVAL, ENOEXEC, ENOMEM, Errno};
use crate::task::{self, Pid, WaitStatus};
use crate::vfs::{self, NodeKind};

/// A pilha cresce para baixo a partir daqui, até `STACK_MAX_SIZE`.
const STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
const STACK_MAX_SIZE: u64 = 8 << 20;
const STACK_INITIAL_SIZE: u64 = 8 * FRAME_SIZE;
/// Limite para o crescimento do heap via `brk`.
const HEAP_MAX_SIZE: u64 = 1 << 30;
/// As áreas de `mmap` são distribuídas a partir daqui.
const MMAP_BASE: u64 = 0x0000_1000_0000_0000;
/// Maior trecho de memória de usuário que uma syscall aceita de uma vez.
const MAX_USER_BUFFER: u64 = 16 << 20;

pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;

// Entradas do vetor auxiliar (auxv) que a libc lê ao iniciar.
const AT_NULL: u64 = 0;
const AT_PAGESZ: u64 = 6;
const AT_ENTRY: u64 = 9;
const AT_RANDOM: u64 = 25;

#[derive(Clone, Copy, PartialEq, Eq)]
enum VmaKind {
    Image,
    Heap,
    Stack,
    Anonymous,
}

#[derive(Clone, Copy)]
struct Vma {
    start: u64,
    end: u64,
    /// Combinação de PROT_READ/WRITE/EXEC.
    prot: u32,
    kind: VmaKind,
}

fn page_flags(prot: u32) -> u64 {
    let mut flags = 0;
    if prot & PROT_WRITE != 0 {
        flags |= WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        flags |= NO_EXECUTE;
    }
    flags
}

fn page_down(addr: u64) -> u64 {
    addr & !(FRAME_SIZE - 1)
}

fn page_up(addr: u64) -> u64 {
    addr.next_multiple_of(FRAME_SIZE)
}

pub struct Process {
    pub space: AddressSpace,
    vmas: Vec<Vma>,
    brk_start: u64,
    brk: u64,
    mmap_next: u64,
    pub fds: FdTable,
    /// Diretório atual, sempre um caminho absoluto normalizado.
    pub cwd: String,
    pub signals: SignalState,
}

impl Process {
    fn vma(&self, addr: u64) -> Option<Vma> {
        self.vmas.iter().find(|v| (v.start..v.end).contains(&addr)).copied()
    }

    /// Tenta resolver uma falha de página. Devolve `false` se o acesso era
    /// mesmo inválido (e o processo deve receber SIGSEGV).
    fn handle_fault(&mut self, addr: u64, error_code: u64) -> bool {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 2;
        const INSTRUCTION: u64 = 16;
        let Some(vma) = self.vma(addr) else { return false };
        if error_code & PRESENT != 0 {
            // Página existe mas é somente-leitura: pode ser cópia sob escrita.
            return error_code & WRITE != 0
                && vma.prot & PROT_WRITE != 0
                && self.space.resolve_copy_on_write(addr);
        }
        if vma.prot == 0
            || (error_code & WRITE != 0 && vma.prot & PROT_WRITE == 0)
            || (error_code & INSTRUCTION != 0 && vma.prot & PROT_EXEC == 0)
        {
            return false;
        }
        self.space.map_new(page_down(addr), page_flags(vma.prot)).is_ok()
    }

    /// Garante que `addr..addr+len` é memória de usuário acessível (para
    /// escrita, se `write`), mapeando o que ainda não foi tocado e separando
    /// as páginas de cópia sob escrita. Depois disso o kernel pode ler ou
    /// escrever ali diretamente.
    pub fn check_range(&mut self, addr: u64, len: u64, write: bool) -> Result<(), Errno> {
        if len == 0 {
            return Ok(());
        }
        let end = addr.checked_add(len).ok_or(EFAULT)?;
        if end > USER_TOP || len > MAX_USER_BUFFER {
            return Err(EFAULT);
        }
        for page in (page_down(addr)..end).step_by(FRAME_SIZE as usize) {
            if let Some((_, flags)) = self.space.translate(page) {
                if write
                    && flags & WRITABLE == 0
                    && !(flags & COPY_ON_WRITE != 0 && self.space.resolve_copy_on_write(page))
                {
                    return Err(EFAULT);
                }
                continue;
            }
            let vma = self.vma(page).ok_or(EFAULT)?;
            if vma.prot == 0 || (write && vma.prot & PROT_WRITE == 0) {
                return Err(EFAULT);
            }
            self.space.map_new(page, page_flags(vma.prot)).map_err(|_| ENOMEM)?;
        }
        Ok(())
    }

    /// `brk`: move o fim do heap e devolve o fim atual (ou o antigo, se o
    /// pedido for inválido), como no Linux.
    pub fn brk(&mut self, requested: u64) -> u64 {
        if requested < self.brk_start || requested > self.brk_start + HEAP_MAX_SIZE {
            return self.brk;
        }
        let new_end = page_up(requested);
        if self.vmas.iter().any(|v| v.kind != VmaKind::Heap && v.start < new_end && self.brk_start < v.end) {
            return self.brk;
        }
        let heap = self.vmas.iter_mut().find(|v| v.kind == VmaKind::Heap).unwrap();
        let old_end = heap.end;
        heap.end = new_end;
        for page in (new_end..old_end).step_by(FRAME_SIZE as usize) {
            self.space.unmap(page);
        }
        self.brk = requested;
        self.brk
    }

    /// `mmap` anônimo: reserva uma região; as páginas vêm sob demanda.
    pub fn mmap(&mut self, len: u64, prot: u32) -> Result<u64, Errno> {
        if len == 0 || len > HEAP_MAX_SIZE {
            return Err(EINVAL);
        }
        let start = self.mmap_next;
        let end = start + page_up(len);
        self.mmap_next = end + FRAME_SIZE; // uma página de guarda entre regiões
        self.vmas.push(Vma { start, end, prot, kind: VmaKind::Anonymous });
        Ok(start)
    }

    pub fn munmap(&mut self, addr: u64, len: u64) -> Result<(), Errno> {
        if addr % FRAME_SIZE != 0 || len == 0 {
            return Err(EINVAL);
        }
        let end = addr.checked_add(page_up(len)).filter(|&e| e <= USER_TOP).ok_or(EINVAL)?;
        let mut kept = Vec::with_capacity(self.vmas.len() + 1);
        for vma in self.vmas.drain(..) {
            if vma.end <= addr || vma.start >= end || vma.kind == VmaKind::Heap {
                kept.push(vma);
                continue;
            }
            if vma.start < addr {
                kept.push(Vma { end: addr, ..vma });
            }
            if vma.end > end {
                kept.push(Vma { start: end, ..vma });
            }
        }
        self.vmas = kept;
        for page in (addr..end).step_by(FRAME_SIZE as usize) {
            self.space.unmap(page);
        }
        Ok(())
    }

    /// A cópia do processo para o `fork`: memória em cópia sob escrita,
    /// descritores apontando para os mesmos arquivos abertos.
    pub fn fork(&mut self) -> Result<Process, Errno> {
        Ok(Process {
            space: self.space.fork().ok_or(ENOMEM)?,
            vmas: self.vmas.clone(),
            brk_start: self.brk_start,
            brk: self.brk,
            mmap_next: self.mmap_next,
            fds: self.fds.clone(),
            cwd: self.cwd.clone(),
            signals: self.signals.clone(),
        })
    }
}

pub fn handle_page_fault(addr: u64, error_code: u64) -> bool {
    task::with_current_process(|p| p.handle_fault(addr, error_code)).unwrap_or(false)
}

/// Um executável carregado num espaço novo, pronto para começar.
struct Image {
    space: AddressSpace,
    vmas: Vec<Vma>,
    brk: u64,
    entry: u64,
    rsp: u64,
}

enum LoadError {
    Format(&'static str),
    NoMemory,
    TooBig,
}

impl LoadError {
    fn errno(&self) -> Errno {
        match self {
            LoadError::Format(_) => ENOEXEC,
            LoadError::NoMemory => ENOMEM,
            LoadError::TooBig => E2BIG,
        }
    }

    fn message(&self) -> &'static str {
        match self {
            LoadError::Format(message) => message,
            LoadError::NoMemory => "memória insuficiente",
            LoadError::TooBig => "argumentos grandes demais",
        }
    }
}

/// Carrega o executável ELF `data` num novo espaço de endereçamento, com
/// `args` como argv e `env` como ambiente.
fn load(data: &[u8], args: &[&str], env: &[&str]) -> Result<Image, LoadError> {
    let image = elf::parse(data).map_err(LoadError::Format)?;
    let mut space = AddressSpace::new().ok_or(LoadError::NoMemory)?;
    let mut vmas = Vec::new();
    let mut image_end = 0;

    for segment in &image.segments {
        let start = page_down(segment.vaddr);
        let end = page_up(segment.vaddr + segment.mem_size);
        let mut prot = 0;
        for (flag, p) in [(PF_R, PROT_READ), (PF_W, PROT_WRITE), (PF_X, PROT_EXEC)] {
            if segment.flags & flag != 0 {
                prot |= p;
            }
        }
        for page in (start..end).step_by(FRAME_SIZE as usize) {
            match space.translate(page) {
                // Página dividida com o segmento anterior: une as permissões.
                Some((_, old)) => {
                    let flags = (old & WRITABLE) | page_flags(prot) & WRITABLE;
                    let nx = if old & NO_EXECUTE != 0 && prot & PROT_EXEC == 0 { NO_EXECUTE } else { 0 };
                    space.set_flags(page, flags | nx);
                }
                None => {
                    space.map_new(page, page_flags(prot)).map_err(|_| LoadError::NoMemory)?;
                }
            }
        }
        let bytes = &data[segment.offset..segment.offset + segment.file_size];
        space.write(segment.vaddr, bytes).map_err(|_| LoadError::NoMemory)?;
        vmas.push(Vma { start, end, prot, kind: VmaKind::Image });
        image_end = image_end.max(end);
    }

    let stack_bottom = STACK_TOP - STACK_MAX_SIZE;
    vmas.push(Vma { start: stack_bottom, end: STACK_TOP, prot: PROT_READ | PROT_WRITE, kind: VmaKind::Stack });
    for page in (STACK_TOP - STACK_INITIAL_SIZE..STACK_TOP).step_by(FRAME_SIZE as usize) {
        space.map_new(page, WRITABLE | NO_EXECUTE).map_err(|_| LoadError::NoMemory)?;
    }
    let rsp = build_initial_stack(&mut space, args, env, image.entry)?;
    vmas.push(Vma { start: image_end, end: image_end, prot: PROT_READ | PROT_WRITE, kind: VmaKind::Heap });
    Ok(Image { space, vmas, brk: image_end, entry: image.entry, rsp })
}

/// Monta a pilha inicial no formato do System V (o que o `_start` de
/// qualquer programa do Linux espera):
///
/// ```text
/// rsp -> argc | argv[0..argc] | NULL | envp... | NULL | auxv (tipo, valor)... | AT_NULL
///        ... e, mais acima, as strings e 16 bytes aleatórios.
/// ```
fn build_initial_stack(space: &mut AddressSpace, args: &[&str], env: &[&str], entry: u64) -> Result<u64, LoadError> {
    let strings_size: usize = args.iter().chain(env).map(|s| s.len() + 1 + 8).sum();
    if strings_size > (STACK_INITIAL_SIZE / 2) as usize {
        return Err(LoadError::TooBig);
    }
    let mut sp = STACK_TOP;
    let mut push_strings = |strings: &[&str], space: &mut AddressSpace| -> Result<Vec<u64>, LoadError> {
        let mut pointers = Vec::with_capacity(strings.len());
        for s in strings {
            sp -= s.len() as u64 + 1;
            space.write(sp, s.as_bytes()).map_err(|_| LoadError::TooBig)?;
            space.write(sp + s.len() as u64, &[0]).map_err(|_| LoadError::TooBig)?;
            pointers.push(sp);
        }
        Ok(pointers)
    };
    let argv = push_strings(args, space)?;
    let envp = push_strings(env, space)?;

    // Semente para a libc (proteção de pilha, hash...). Não é criptográfica.
    sp -= 16;
    let random = sp;
    let seed = cpu::rdtsc().wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    bytes[8..].copy_from_slice(&seed.rotate_left(29).wrapping_mul(0xBF58_476D_1CE4_E5B9).to_le_bytes());
    space.write(random, &bytes).map_err(|_| LoadError::TooBig)?;

    let mut words = vec![args.len() as u64];
    words.extend(&argv);
    words.push(0);
    words.extend(&envp);
    words.push(0);
    words.extend([AT_PAGESZ, FRAME_SIZE, AT_RANDOM, random, AT_ENTRY, entry, AT_NULL, 0]);

    // O rsp inicial (apontando para argc) precisa estar alinhado a 16.
    sp &= !0xF;
    if words.len() % 2 == 1 {
        sp -= 8;
    }
    sp -= words.len() as u64 * 8;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    space.write(sp, &bytes).map_err(|_| LoadError::TooBig)?;
    Ok(sp)
}

fn file_contents(cwd: &str, path: &str) -> Result<Vec<u8>, Errno> {
    match &vfs::resolve(cwd, path)?.kind {
        NodeKind::File(data) => Ok(data.lock().clone()),
        _ => Err(EACCES),
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Inicia um processo a partir do monitor do kernel: ele lidera sua própria
/// sessão e grupo, com 0, 1 e 2 no terminal.
/// Cria um processo filho do kernel, líder de uma sessão nova. Com
/// `foreground`, a sessão recebe o terminal antes de o processo poder rodar
/// (senão um shell poderia se ver em segundo plano e parar com SIGTTIN).
pub fn spawn(path: &str, args: &[&str], env: &[&str], foreground: bool) -> Result<Pid, &'static str> {
    let data = file_contents("/", path).map_err(|_| "programa não encontrado")?;
    let image = load(&data, args, env).map_err(|e| e.message())?;
    let process = Process {
        space: image.space,
        vmas: image.vmas,
        brk_start: image.brk,
        brk: image.brk,
        mmap_next: MMAP_BASE,
        fds: FdTable::with_console(),
        cwd: "/home".to_string(),
        signals: SignalState::new(),
    };
    Ok(cpu::without_interrupts(|| {
        let pid = task::spawn_user(basename(path), process, image.entry, image.rsp);
        if foreground {
            crate::tty::set_foreground(pid, pid);
        }
        pid
    }))
}

/// `execve`: troca a imagem do processo atual. Só volta em caso de erro; se
/// der certo, `frame` passa a apontar para o início do novo programa.
pub fn exec(frame: &mut InterruptFrame, path: &str, args: &[&str], env: &[&str]) -> Result<(), Errno> {
    let cwd = task::with_current_process(|p| p.cwd.clone()).ok_or(EFAULT)?;
    let data = file_contents(&cwd, path)?;
    let image = load(&data, args, env).map_err(|e| e.errno())?;
    let (entry, rsp) = (image.entry, image.rsp);

    // Daqui em diante não há volta. O espaço antigo só pode ser liberado
    // depois que o CR3 já aponta para o novo.
    let (old_space, closed) = task::with_current_process(|p| {
        unsafe { cpu::write_cr3(image.space.p4()) };
        let old = core::mem::replace(&mut p.space, image.space);
        p.vmas = image.vmas;
        p.brk_start = image.brk;
        p.brk = image.brk;
        p.mmap_next = MMAP_BASE;
        p.signals.reset_for_exec();
        (old, p.fds.take_close_on_exec())
    })
    .ok_or(EFAULT)?;
    drop(old_space);
    drop(closed);

    task::exec_current(basename(path));
    FpuState::clean().restore();
    *frame = InterruptFrame {
        rip: entry,
        cs: gdt::USER_CODE as u64,
        rflags: 0x202,
        rsp,
        ss: gdt::USER_DATA as u64,
        ..InterruptFrame::default()
    };
    Ok(())
}

/// Encerra o processo atual. Roda no contexto dele, então os arquivos são
/// fechados aqui (e quem estiver do outro lado de um pipe é avisado já).
pub fn exit_current(status: WaitStatus) {
    let descriptors = task::with_current_process(|p| p.fds.take_all());
    drop(descriptors);
    task::exit_current(status);
}
