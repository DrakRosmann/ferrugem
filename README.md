# FerrugemOS

Um sistema operacional tipo Unix para x86_64, escrito do zero em Rust (com um
pouco de assembly). Kernel com memória virtual (com copy-on-write), processos,
multitarefa preemptiva, sinais, controle de jobs, usuários e permissões,
links simbólicos e físicos, chamadas de sistema
compatíveis com as do Linux, pipes, sistema de arquivos e driver de disco ext2
— e, em cima dele, `login`, um shell e mais de 30 programas em modo usuário.
Programas em C compilados com a **musl** (a libc do Alpine Linux) também rodam.

```
ferrugem login: usuario
Senha:
usuario@ferrugem:~$ cat /etc/shadow
cat: /etc/shadow: permissão negada
usuario@ferrugem:~$ echo "olá" > /disco/nota.txt        # sobrevive ao desligar
usuario@ferrugem:~$ ola                                 # um programa em C, com a musl
Olá do C! Rodando em FerrugemOS 0.4.0 (x86_64), pid 4
usuario@ferrugem:~$ su
Senha:
root@ferrugem:/home/usuario# ps
  PID   PAI  USUÁRIO   ESTADO      MEMÓRIA     CPU  NOME
    0     0  root      pronto         0 KiB    8.71s  ocioso
    1     0  root      dormindo     408 KiB    0.02s  init
    2     1  usuario   dormindo     444 KiB    0.03s  sh
    3     2  root      dormindo     444 KiB    0.03s  sh
    9     3  root      rodando      416 KiB    0.00s  ps
```

## Como rodar (Fedora / Arch)

```bash
# Fedora
sudo dnf install nasm qemu-system-x86 binutils make lld e2fsprogs rust-std-static-x86_64-unknown-none
# Arch
sudo pacman -S nasm qemu-desktop binutils make lld e2fsprogs
rustup target add x86_64-unknown-none        # se o Rust veio do rustup

make run        # janela do QEMU + terminal na serial (dá para digitar nos dois)
make run-nox    # só no terminal; para sair: Ctrl+A e depois X
make teste      # boota, roda os testes do kernel e da libc, desliga e confere o disco com e2fsck
make disco      # recria o disco (disco.img) do zero

make musl       # baixa (de musl.libc.org, conferindo o SHA-256) e compila a musl:
                # depois disso os programas em c/ entram no sistema
make lua        # baixa o Lua 5.4.9 de lua.org, confere, compila sem modificar
                # nada e põe em /bin/lua
```

Usuários de teste: **root** (senha `root`) e **usuario** (senha `usuario`).
Dentro do sistema, digite `ajuda`. O teclado é ABNT2 (com acentos) e o Ctrl+C
interrompe o programa que está rodando.

## O que já existe

**Kernel**
- Boot Multiboot → modo longo (64 bits) → Rust; GDT, TSS e IDT próprias.
- Exceções da CPU: no kernel, tela de pânico com os registradores; num
  programa, só ele morre, com o sinal certo (SIGSEGV, SIGFPE, SIGILL).
- PIC 8259, timer PIT a 100 Hz, relógio CMOS (data e hora), teclado PS/2 ABNT2, serial.
- **Memória:** alocador de quadros físicos (bitmap), heap do kernel (`Box`, `Vec`,
  `Arc`...), um espaço de endereçamento por processo, isolado do kernel e
  descrito por *regiões*: paginação sob demanda (a pilha cresce sozinha até
  8 MiB), `mmap` anônimo e de arquivos (privado ou compartilhado, com `msync`),
  `mprotect`, NX, ASLR, SMEP/SMAP e um "OOM killer" simples.
- **Processos:** `fork` com **copy-on-write**, `execve` (carregador ELF, com
  argumentos, ambiente e vetor auxiliar como no Linux), `exit`, `wait`, órfãos
  adotados pelo init, escalonador round-robin **preemptivo**. Registradores
  SSE/FPU e TLS (FS) salvos por processo.
- **Threads:** `clone` com `CLONE_THREAD` (memória, arquivos, diretório e
  tratadores divididos), `futex` (WAIT, WAKE, REQUEUE, com prazo; a chave é o
  endereço físico), `set_tid_address`/`CLONE_CHILD_CLEARTID`, `gettid`,
  `tgkill`, `exit_group`. As pthreads da musl funcionam: mutex, variável de
  condição, `__thread`, `pthread_join`.
- **Sinais:** tratadores próprios (`sigaction`, `rt_sigreturn`), máscara
  (`sigprocmask`), `SA_RESTART`, `alarm`, `pause`, `kill`, SIGCHLD, SIGPIPE.
- **Controle de jobs:** grupos de processos e sessões (`setpgid`, `setsid`),
  terminal com grupo em primeiro plano (`tcsetpgrp`), processos parados
  (SIGSTOP/SIGTSTP/SIGCONT, `wait` com `WUNTRACED`), SIGTTIN para quem lê o
  teclado em segundo plano, `kill` para um grupo inteiro.
- **Usuários e permissões:** uid/gid real e efetivo, `rwx` + setuid, setgid e
  sticky em todo arquivo (memória e ext2), `chmod`, `chown`, `umask`, `setuid`.
- **`/proc`** (gerado na hora): `self`, `<pid>/status|stat|cmdline|cwd|exe`,
  `meminfo`, `uptime`, `cpuinfo`, `mounts`, `loadavg`. `/dev/random` e
  `/dev/urandom` com a RDRAND da CPU, quando existe. Montagens genéricas.
- **Syscalls** pela instrução `syscall` (ou `int 0x80`), com os mesmos números
  e estruturas do Linux — umas 100, o suficiente para a musl: `poll`, `select`, `read`, `writev`,
  `openat`, `newfstatat`, `mmap`, `brk`, `clone` (do jeito do `posix_spawn`),
  `fcntl`, `pipe2`, `dup3`, `ioctl` de terminal, `arch_prctl`, `getrandom`...
  Ponteiros inválidos viram `EFAULT`.
- **Arquivos:** VFS com descritores, posição, pipes e dispositivos (`/dev/tty`,
  `/dev/null`, `/dev/zero`). A raiz fica em memória (vinda do initrd) e
  `/disco` é um **disco ext2 de verdade** (driver ATA PIO), que persiste e que o
  Linux consegue ler. Links simbólicos (curtos dentro do inode, longos em bloco,
  como no Linux) e físicos, nos dois.
- **Terminal:** edição de linha no kernel, `termios` (eco e modo canônico
  ligáveis), Ctrl+C (SIGINT) e Ctrl+Z (SIGTSTP) só para o job em primeiro
  plano, Ctrl+D vira fim de arquivo, cores ANSI tanto na VGA quanto na serial.

**Modo usuário** (`usuario/`)
- `ulib`: a biblioteca dos programas em Rust — `_start`, syscalls, `malloc` com
  `brk`, `print!`, sinais, ambiente, usuários, SHA-256.
- `init` (processo 1) → `login` → `sh`: pipes `|`, `>`, `>>`, `<`, `&`, `;`,
  `$?`, `$VARIAVEL`, `~`, aspas, comentários, scripts, `sh -c`, `export`, `cd`,
  e controle de jobs: Ctrl+Z, `jobs`, `fg`, `bg`.
- `rastro` (um strace: `rastro ls`, `rastro -c lua script.lua`), `login su passwd id whoami chmod chown env ln readlink ls cat echo cp mv rm mkdir rmdir
  touch grep wc head ps kill sleep date uptime free df uname clear desligar
  reiniciar teste`.
- **Em C, com a musl** (`c/`): `ola` e `testec`, a bateria de testes da libc
  (stdio, `malloc` de 8 MiB, `double`, arquivos, `fork`+`execl`, `system`, `popen`, sinais, tempo).

## Estrutura

| Onde | O quê |
|---|---|
| `boot/boot.asm` | Header Multiboot, checagem da CPU, tabelas de página iniciais, salto para 64 bits |
| `src/interrupcoes.asm` | 256 pontos de entrada de interrupção, `troca_contexto`, entrada do `syscall` |
| `src/lib.rs` | `kmain`: a ordem de inicialização |
| `src/gdt.rs`, `interrupcoes.rs` | GDT/TSS, IDT, tratador central, exceções |
| `src/pic.rs`, `pit.rs`, `rtc.rs` | Controlador de interrupções, timer, relógio |
| `src/quadros.rs`, `heap.rs`, `paginacao.rs` | Memória física, heap do kernel, memória virtual |
| `src/processo.rs` | Processos, escalonador, `fork`/`exec`/`exit`/`wait`, sinais |
| `src/syscall.rs` | Todas as chamadas de sistema |
| `src/elf.rs`, `initrd.rs` | Carregador de executáveis, leitor de tar |
| `src/vfs.rs`, `arquivo.rs` | Sistema de arquivos virtual, arquivos abertos, pipes |
| `src/ata.rs`, `ext2.rs` | Driver de disco e sistema de arquivos ext2 |
| `src/tty.rs`, `teclado.rs`, `serial.rs`, `vga.rs` | Terminal e dispositivos de entrada e saída |
| `comum/lista_livre.rs` | Alocador usado pelo kernel e pelos programas |
| `usuario/src/lib.rs` + `sys.rs`, `io.rs` | `ulib`, a biblioteca dos programas |
| `usuario/src/bin/*.rs` | Um programa por arquivo, todos vão para `/bin` |
| `raiz/` | Arquivos que vão para o initrd (`/etc/motd`, `/home`...) |
| `disco-inicial/` | Conteúdo inicial do disco ext2 |
| `src/sinal.rs` | Sinais: ações, máscara, quadro do tratador na pilha do usuário |
| `usuario/src/usuarios.rs`, `sha256.rs` | /etc/passwd, /etc/group, /etc/shadow e o hash das senhas |
| `c/` | Programas em C, compilados com a musl |
| `ferramentas/` | Scripts que baixam e compilam a musl e o Lua |
| `testes/rodar.sh` | O `make teste` |

## Como as peças se encaixam

### Memória de cada processo

```
0       .. 1 GiB      kernel: o mesmo em todos, mapeado só para o anel 0
1 GiB   .. ...        programa (ELF) e logo depois o heap, que cresce com brk
...     .. 508 GiB    pilha do usuário (256 KiB), crescendo para baixo
```

### Uma chamada de sistema

```
programa: mov rax, 1 (write) ; syscall
  └► entrada_syscall (asm): troca para a pilha de kernel do processo,
     monta o mesmo quadro de uma interrupção ─► isr_comum: salva tudo
       └► tratar_interrupcao ─► syscall::tratar ─► sys_write ─► Arquivo::escrever
     ◄─ RAX = bytes escritos (ou -errno) ◄─ iretq de volta ao anel 3
```

### Multitarefa

Cada processo tem sua própria pilha de kernel. Trocar de processo é trocar
de pilha (`troca_contexto`). O timer interrompe o programa 100 vezes por
segundo e, na volta para o modo usuário, o escalonador pode escolher outro.
Quem espera alguma coisa (teclado, pipe, filho, tempo) dorme num "canal" e
é acordado quando o evento acontece.

O kernel roda sempre com interrupções desligadas (elas só ficam ligadas em modo
usuário e no processo ocioso). Numa CPU só, isso faz o kernel inteiro funcionar
como uma grande seção crítica, a mesma ideia do "big kernel lock" das primeiras
versões do Linux.

## Próximos passos

1. **Portar mais programas de verdade:** o Lua 5.4 já roda sem modificações
   (`make lua`); depois, `dash` e `busybox`. O kernel avisa quando falta uma
   syscall: `chamada de sistema N não implementada`.
2. **Hardware moderno:** ACPI, APIC/IOAPIC, SMP (várias CPUs — exige trocar o
   "big kernel lock" por locks de verdade), boot UEFI, framebuffer.
3. **Rede:** driver virtio-net ou e1000, Ethernet, ARP, IP, ICMP, UDP, TCP, sockets.
4. **Interface gráfica:** mouse, servidor de janelas.

Boas referências: [Writing an OS in Rust](https://os.phil-opp.com/),
[OSDev Wiki](https://wiki.osdev.org/), o livro do [xv6](https://pdos.csail.mit.edu/6.828/2024/xv6/book-riscv-rev4.pdf)
(MIT), [Redox OS](https://www.redox-os.org/) e a [especificação do ext2](https://www.nongnu.org/ext2-doc/ext2.html).

## Licença

GPL-3.0-or-later (veja o arquivo `LICENSE`).
