# FerrugemOS

Um sistema operacional x86_64 escrito do zero em Rust: sem `std` e sem
nenhum crate externo. O kernel tem memória virtual, processos isolados no
anel 3 com `fork`/`execve`, pipes, sinais, um terminal com controle de jobs
e chamadas de sistema compatíveis com as do Linux, de modo que binários
estáticos simples feitos para o Linux rodam nele sem mudança. No boot ele
abre o `/bin/sh`, um shell Unix escrito para ele.

## Requisitos (Fedora)

```sh
sudo dnf install rust-std-static-x86_64-unknown-none qemu-system-x86 binutils gcc
# opcional, para gerar a ISO: grub2-tools-extra grub2-pc-modules xorriso
```

Não precisa de nightly: tudo compila no Rust estável.

## Rodando

```sh
make run          # janela do QEMU (tela VGA) + serial no terminal
make run-serial   # sem janela: o shell roda no próprio terminal
cargo run --release   # o mesmo que `make run`
make iso          # target/ferrugem.iso, inicializável via GRUB
```

Para testar: `make test`. Cada arquivo em `tests/` é uma sessão no `sh`
(comandos e a saída esperada); o `scripts/testar.py` dá boot no QEMU,
digita cada comando quando o prompt aparece e compara a saída. O GitHub
Actions roda os mesmos testes a cada push.

Para sair do QEMU sem janela, digite `exit` no sh e depois `desligar` no
monitor do kernel (ou Ctrl+A e depois X).

## Experimente

No `sh` (`ajuda` resume a sintaxe):

```text
$ ls /bin                          # os arquivos em verde são programas
$ ola a b c                        # roda no anel 3 e mostra pid, argumentos, CS
$ cat leiame.txt | grep -c sistema # pipes
$ echo oi > /tmp/a; echo tchau >> /tmp/a; wc -l < /tmp/a
$ ls /nada 2>&1 | head -n 1        # redirecionamento de erros
$ make || echo "falhou com $?"     # listas: ; && ||
$ pi 100000000 &                   # segundo plano; rode três e veja `jobs`
$ yes > /dev/null                  # Ctrl+Z para; depois bg, jobs, kill %1
$ sleep 30                         # Ctrl+Z, fg, Ctrl+C
$ falha                            # lê a memória do kernel e morre (SIGSEGV)
$ sh -c 'echo $0 $1' a b           # também roda scripts: sh arquivo args...
$ exit                             # volta ao monitor do kernel
```

No monitor do kernel (`ajuda` lista os comandos):

```text
ferrugem> ps                 # anel, grupo, CPU e memória residente de cada tarefa
ferrugem> memoria 30000 &    # heap, mmap e pilha sob demanda; veja no `ps`
ferrugem> sh                 # reabre o shell
```

Os mesmos binários de `user/` rodam no Linux:
`target/user/x86_64-unknown-linux-gnu/release/ola`.

## Como funciona

### Memória virtual

```text
0x0000_0000_0000_0000 .. 0x0000_8000_0000_0000   processo (um espaço por processo)
0xFFFF_8000_0000_0000 .. +4 GiB                   mapa direto da RAM física
0xFFFF_FFFF_8000_0000 .. +1 GiB                   imagem do kernel
```

O boot entra no modo longo com um mapeamento 1:1 temporário, salta para a
metade alta e o kernel desfaz o 1:1 (um ponteiro nulo no kernel passa a
dar falha de página). Cada processo tem sua tabela P4; a metade alta é
compartilhada, mas sem o bit USER. A memória de um processo é descrita por
regiões (imagem do ELF, heap, pilha, `mmap`), e as páginas só são criadas
quando tocadas, no tratador de falha de página. Acesso fora das regiões, ou
contra a permissão delas, encerra o processo com SIGSEGV.

### Processos e chamadas de sistema

- O carregador lê ELF64 estáticos (`ET_EXEC`), monta a pilha inicial no
  formato System V (argc, argv, envp, auxv) e entra no anel 3 com `iretq`.
- `fork` compartilha as páginas com cópia sob demanda (copy-on-write) e
  copia a tabela de descritores; `execve`
  troca a imagem mantendo os descritores (menos os `O_CLOEXEC`); `wait4`
  colhe filhos que terminaram ou pararam (`WUNTRACED`).
- A entrada de `syscall` troca para a pilha de kernel da tarefa e monta o
  mesmo frame de uma interrupção. Assim, uma syscall pode bloquear (`read`,
  `nanosleep`) ou trocar de tarefa (`exit`, `sched_yield`) pelo mesmo
  caminho do escalonador.
- Na troca de tarefa mudam junto o CR3, o estado da FPU/SSE (`fxsave`), a
  base do FS (TLS) e o rsp0 do TSS.
- Sinais POSIX: pendentes por tarefa, com máscara, `rt_sigaction` e
  tratadores no anel 3 (quadro na pilha do usuário + `rt_sigreturn`).
- Syscalls implementadas, com os números do Linux:
  - arquivos: `read`, `write`, `readv`, `writev`, `open`, `openat`,
    `close`, `lseek`, `stat`, `fstat`, `lstat`, `newfstatat`,
    `getdents64`, `dup`, `dup2`, `dup3`, `fcntl`, `pipe`, `pipe2`,
    `ioctl`, `getcwd`, `chdir`, `mkdir`, `mkdirat`, `rmdir`, `unlink`,
    `unlinkat`, `access`, `faccessat`;
  - processos: `fork`, `vfork`, `clone` (só a forma do `fork`), `execve`,
    `exit`, `exit_group`, `wait4`, `kill`, `getpid`, `getppid`, `gettid`,
    `setpgid`, `getpgid`, `getpgrp`, `setsid`, `getsid`, `getuid` e afins;
  - sinais: `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`;
  - memória e outras: `mmap` (anônimo), `munmap`, `brk`, `sched_yield`,
    `nanosleep`, `uname`, `arch_prctl`, `set_tid_address`,
    `clock_gettime`.

  As que faltam devolvem ENOSYS e são registradas na serial.

### Terminal e shell

O terminal (`tty.rs`) tem disciplina de linha com `termios` (modo canônico
ou cru, eco), uma sessão e um grupo em primeiro plano: Ctrl+C, Ctrl+Z e
Ctrl+\\ viram SIGINT, SIGTSTP e SIGQUIT para esse grupo, e um grupo em
segundo plano que tenta ler recebe SIGTTIN.

O `/bin/sh` (`user/src/bin/sh.rs`) separa a linha em tokens respeitando
aspas, monta listas (`;`, `&&`, `||`, `&`) de pipelines de comandos com
redirecionamentos, e só expande `$VAR`, `$?`, `~` etc. na hora de executar
cada comando. Cada pipeline vira um job num grupo de processos próprio,
que recebe o terminal (`tcsetpgrp`) enquanto roda em primeiro plano.
Comandos internos: `cd`, `pwd`, `export`, `unset`, `jobs`, `fg`, `bg`,
`wait`, `kill` (aceita `%n`), `exit` e `ajuda`.

### Arquivos

| Arquivo | Papel |
|---|---|
| `src/boot.s` | Cabeçalho Multiboot; tabelas de página iniciais, modo longo, salto para a metade alta |
| `src/main.rs` | `kernel_main`: ordem de inicialização, barra de status, handler de pânico |
| `src/paging.rs` | Tabelas de 4 níveis, `AddressSpace` por processo, mapa direto |
| `src/memory.rs`, `src/multiboot.rs` | Mapa de memória do carregador e alocador de quadros físicos (bitmap) |
| `src/heap.rs` | `#[global_allocator]` do kernel (lista livre com fusão de blocos) |
| `src/process.rs`, `src/elf.rs` | Carregador de ELF, regiões de memória, paginação sob demanda, `brk`/`mmap` |
| `src/syscall/` | Entrada de `syscall` e despacho (`mod.rs`); syscalls de arquivos (`fs.rs`) e de processos e sinais (`proc.rs`) |
| `src/task.rs` | Escalonador preemptivo: threads do kernel e processos, zumbis, `wait4`, grupos e sessões |
| `src/signal.rs` | Sinais: ações, máscara, entrega no retorno ao anel 3 e `rt_sigreturn` |
| `src/gdt.rs` | Segmentos do kernel e do usuário, TSS (rsp0 e pilha IST da falta dupla) |
| `src/isr.s`, `src/interrupts.rs` | 256 stubs de interrupção, IDT, PIC 8259, exceções (que matam só o processo culpado) |
| `src/tty.rs`, `src/input.rs` | Terminal: termios, disciplina de linha, grupo em primeiro plano, Ctrl+C/Z; fila de entrada |
| `src/keyboard.rs`, `src/serial.rs`, `src/timer.rs` | Teclado PS/2 (layout US), UART 16550, PIT a 100 Hz |
| `src/vga.rs`, `src/console.rs` | Modo texto 80x25 com cores e `print!` espelhado na serial |
| `src/vfs.rs`, `src/file.rs`, `src/pipe.rs` | Sistema de arquivos em memória (`/bin`, `/dev`, `/home`, `/tmp`); arquivos abertos e descritores; pipes |
| `src/shell.rs` | Monitor do kernel: inicia o `/bin/sh` e tem comandos de diagnóstico |
| `src/sync.rs`, `src/cpu.rs`, `src/power.rs` | Trava giratória sem interrupções; instruções de baixo nível; reiniciar e desligar |
| `user/` | Biblioteca de usuário (`_start`, syscalls, `print!`, alocador sobre `brk`) e programas: `sh`, `ls`, `cat`, `grep`, `wc`, `head`, `echo`, `kill`, `sleep`, `yes`... |

O `build.rs` compila `user/` para `x86_64-unknown-linux-gnu` (binários
estáticos, sem libc) e os programas em `user/c/` com o gcc, e embute tudo
no kernel; no boot eles são instalados no sistema de arquivos.

O kernel é entregue ao carregador como binário plano (`objcopy -O binary`)
com os endereços físicos no cabeçalho Multiboot, porque o ELF ligado na
metade alta não cabe no formato de 32 bits que o `-kernel` do QEMU aceita.

## Escrevendo um programa

Crie `user/src/bin/meuprog.rs`:

```rust
#![no_std]
#![no_main]

extern crate alloc;

use ferrugem_user::{entry, println, rt};

entry!(main);

fn main() -> i32 {
    println!("argumentos: {:?}", rt::args().collect::<alloc::vec::Vec<_>>());
    0
}
```

Depois rode `make run` e digite `meuprog` no shell.

## Próximos passos

- `/proc`, para o `ps` virar um programa de usuário
- Subshells `( ... )`, substituição de comandos `$(...)` e globs (`*.txt`) no sh
- Executáveis PIE/estáticos com musl, para rodar o busybox
- Um disco de verdade (ATA/virtio) e um sistema de arquivos persistente

## Licença

Copyright (C) 2026 DrakRosmann

Este programa é software livre: você pode redistribuí-lo e/ou modificá-lo
sob os termos da GNU General Public License, como publicada pela Free
Software Foundation, na versão 3 da licença ou (a seu critério) qualquer
versão posterior. Ele é distribuído na esperança de ser útil, mas SEM
NENHUMA GARANTIA. Veja o texto completo em [LICENSE](LICENSE).
