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
make run-sistema   # boota com a RAIZ NO DISCO: o sistema inteiro num ext2 (sistema.img),
                   # onde tudo o que você muda (senhas, /home...) fica
make sistema    # recria o sistema.img do zero

make musl       # baixa (de musl.libc.org, conferindo o SHA-256) e compila a musl:
                # depois disso os programas em c/ entram no sistema
make lua        # baixa o Lua 5.4.9 de lua.org, confere, compila sem modificar
                # nada e põe em /bin/lua
```

Usuários de teste: **root** (senha `root`) e **usuario** (senha `usuario`).
Dentro do sistema, digite `ajuda`. O teclado é ABNT2 (com acentos) e o Ctrl+C
interrompe o programa que está rodando. Alt+F1..F4 troca de console.

**Interface gráfica:** com `make run`, entre na janela do QEMU e digite
`janelas`. Clique na janela do QEMU para ele capturar o mouse (Ctrl+Alt+G
solta). O menu **Aplicativos** (no alto) abre terminais e programas e tem o
"Sair". As janelas são arrastadas pelo título, redimensionadas pelo canto de
baixo, minimizadas e maximizadas pelos botões (ou com dois cliques no
título); a barra de tarefas, embaixo, alterna entre elas, e Alt+Tab também.
No terminal, arrastar copia e o botão direito cola; nos programas, Ctrl+C e
Ctrl+V. No canto direito do painel fica o **painel de controle**: brilho,
estilo escuro, luz noturna, segundos no relógio, cursor grande, captura de
tela (PNG em `~/`), bloquear a tela (pede a senha) e desligar. Se algo
travar, Ctrl+Alt+Backspace derruba a interface gráfica.

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
- **Discos:** os 4 discos IDE e as partições MBR (`/dev/hda`, `/dev/hdb1`...,
  legíveis cruas pelo root); `mount`/`umount` de ext2, `tmpfs` e `proc` em
  qualquer diretório; cache de disco com escrita adiada (vai para o disco no
  `sync`, ao desmontar, ao desligar ou a cada 3 s); `statfs` e `df`;
  `truncate` para qualquer tamanho; FIFOs (`mkfifo`) na memória e no ext2.
- **Comunicação entre processos:** pipes, FIFOs, **sockets Unix** (stream e
  datagrama; endereço no sistema de arquivos, inclusive no ext2, ou
  abstrato; `socketpair`, `SO_PEERCRED`, `shutdown`), **memória
  compartilhada** do System V (`shmget`/`shmat`, páginas sob demanda) e
  **epoll**; `O_NONBLOCK` pelo `fcntl` em pipes, terminais e sockets.
- **Raiz no disco** (`raiz=/dev/hda` na linha de comando do kernel): o `/` vem
  de um ext2, com `/dev`, `/proc` e um `/tmp` em memória montados por cima,
  como numa distribuição Linux. Sem a opção, o `/` é o initrd na memória.
- **Terminal:** edição de linha no kernel, `termios` (eco e modo canônico
  ligáveis), Ctrl+C (SIGINT) e Ctrl+Z (SIGTSTP) só para o job em primeiro
  plano, Ctrl+D vira fim de arquivo, cores ANSI tanto na VGA quanto na serial.
- **Consoles virtuais:** quatro (`/dev/tty1` a `tty4`), cada um com o seu
  login; **Alt+F1..F4** troca. A serial é o tty1.
- **Pseudo-terminais:** `/dev/ptmx` e `/dev/pts/N` (como no Linux), a base
  para terminais gráficos e acesso remoto. Terminal de controle por sessão:
  `/dev/tty`, `TIOCSCTTY`, `O_NOCTTY`; quando o líder da sessão sai (ou o
  mestre do pseudo-terminal fecha), o primeiro plano recebe **SIGHUP**.

- **Interface gráfica:** framebuffer da placa Bochs VBE do QEMU (1024×768,
  32 bits) em `/dev/fb0`, com os `ioctl` do Linux e `mmap` direto da memória
  de vídeo; mouse PS/2 (IRQ 12) em `/dev/mouse`; captura do teclado em
  `/dev/teclado`; a fonte 8×16 lida da própria VGA (`/dev/fonte`). Ao fechar
  o framebuffer, o kernel repõe os registradores, a fonte e o texto do modo
  texto.
- **Servidor de janelas** (`janelas`), em modo usuário: painel com o menu
  Aplicativos e relógio, barra de tarefas, janelas que se arrastam,
  redimensionam, minimizam e maximizam, Alt+Tab, área de transferência
  (copiar e colar entre terminais e programas), e redesenho só do que
  mudou. Terminais (emulador com cores ANSI ligado a um pseudo-terminal
  com um `sh`, que recebe SIGWINCH ao mudar de tamanho; seleção com o
  mouse) e janelas de outros programas, que conversam com o servidor por
  um socket Unix e desenham numa memória compartilhada (`ulib::janela`;
  exemplos: `pintar`, `fontes`, `componentes`).
- **Aplicativos:** `arquivos` (gerenciador de arquivos: tabela com ícones,
  pastas primeiro, clique duplo abre — pasta, imagem, programa ou texto —,
  criar pasta, renomear, apagar, copiar e colar) e `editor` (editor de
  texto com números de linha, seleção pelo mouse, área de transferência,
  Ctrl+S/O/N/A). Ambos no menu Aplicativos.
- **Painel de controle** (no estilo das configurações rápidas do GNOME):
  brilho e luz noturna (um filtro de cor aplicado pelo servidor ao copiar
  para a tela), estilo escuro (os programas com `ulib::ui` trocam de tema na
  hora), segundos no relógio, cursor grande, captura de tela num PNG
  (`ulib::png`), tela de bloqueio com senha, sair/reiniciar/desligar, e o
  uso de memória. Os ajustes ficam em `~/.janelas`. Sem rede, som e
  bateria no sistema, não há botões de mentira para eles.
- **Componentes de interface** (`ulib::ui`, em "modo imediato", como a Dear
  ImGui): rótulos, botões, caixas de seleção, opções, campo de texto,
  lista com rolagem (roda do mouse, barra, setas), deslizante, barra de
  progresso, barra de menus com menus suspensos e caixas de diálogo
  modais; foco pelo teclado (Tab, Enter, espaço, setas). O `componentes`
  mostra todos. O mouse PS/2 ganhou a roda (modo IntelliMouse).
- **Texto:** a fonte da VGA ganha no boot as letras acentuadas que faltam
  (ã, õ, Á, Â, Ã, Ê, Í, Ó, Ô, Õ, Ú), geradas no estilo dela: consoles de
  texto e terminais da interface mostram o português inteiro. E fontes
  **TrueType** de qualquer tamanho, com bordas suaves (`ulib::truetype`: um
  leitor de .ttf e um rasterizador por acumulação de área, feitos aqui),
  usadas no painel e nos títulos; o `make` copia DejaVu Sans e Liberation
  Mono do sistema de quem compila, se houver.

- **Várias CPUs (SMP):** x2APIC, CPUs achadas pela MADT do ACPI e acordadas
  por INIT-SIPI-SIPI (`trampolim.asm`), dados por CPU com `swapgs`, timer do
  APIC em cada uma; uma trava para o kernel e programas em paralelo
  (`getcpu`, `/proc/cpuinfo`).
- **Bibliotecas dinâmicas:** o kernel carrega executáveis PIE (`ET_DYN`) numa
  base sorteada e, se o programa pede um carregador (`PT_INTERP`), carrega os
  dois e começa por ele, com o vetor auxiliar completo (`AT_BASE`,
  `AT_ENTRY`, `AT_PHDR`, `AT_EXECFN`...). O carregador é nosso:
  **`/lib/ld-ferrugem.so`** (`dinamico/`, em C sem libc) acha as bibliotecas
  de `DT_NEEDED` em `/lib`, resolve os símbolos pela tabela hash do ELF,
  aplica as relocações (`RELATIVE`, `64`, `GLOB_DAT`, `JUMP_SLOT`, `COPY`),
  roda os construtores e pula para o programa. Exemplo: `dinamico` usa a
  `libsaudacao.so`; `ldd` lista as bibliotecas de um programa. Com a musl
  compartilhada (`make musl`), o carregador dela também funciona, sem
  modificações: `/lib/ld-musl-x86_64.so.1` e o `ola-dinamico`, um programa C
  comum ligado à `libc.so`.

**Modo usuário** (`usuario/`)
- `ulib`: a biblioteca dos programas em Rust — `_start`, syscalls, `malloc` com
  `brk`, `print!`, sinais, ambiente, usuários, SHA-256.
- `init` (processo 1) roda o `/etc/rc` (que inicia os serviços) e abre um
  `login` em cada console → `sh`: pipes `|`, `>`, `>>`, `<`, `&`, `;`,
  `$?`, `$VARIAVEL`, `~`, aspas, comentários, scripts, `sh -c`, `export`, `cd`,
  e controle de jobs: Ctrl+Z, `jobs`, `fg`, `bg`.
- **Serviços:** `syslogd` (recebe pelo socket `/dev/log` e grava em
  `/var/log/mensagens`; o `syslog()` da musl funciona), `logger`, `dmesg`
  (as mensagens do kernel, guardadas num buffer circular), e o **cron**:
  `crond` com `/etc/crontab` e as tabelas de cada usuário, editadas com o
  `crontab` (setuid root); `*/5`, `1-5`, listas, `@reboot`, `@daily`...
  A saída das tarefas vai para o registro. Os logins também são registrados.
- `rastro` (um strace: `rastro ls`, `rastro -c lua script.lua`), `mount umount sync df mkfifo truncate tty`, `login su passwd id whoami chmod chown env ln readlink ls cat echo cp mv rm mkdir rmdir
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
| `src/tty.rs`, `teclado.rs`, `serial.rs`, `vga.rs` | Consoles, pseudo-terminais e dispositivos de entrada e saída |
| `src/socket.rs`, `epoll.rs`, `shm.rs` | Sockets Unix, epoll, memória compartilhada |
| `src/smp.rs`, `trampolim.asm` | Várias CPUs: APIC, ACPI, acordar as outras, a trava do kernel |
| `src/video.rs`, `mouse.rs`, `registro.rs` | Modo gráfico (Bochs VBE), mouse PS/2, registro do kernel (dmesg) |
| `usuario/src/grafico.rs`, `janela.rs` | Desenho (retângulos, linhas, texto) e o lado do cliente das janelas |
| `usuario/src/truetype.rs` | Fontes TrueType: leitor de .ttf e rasterizador com suavização |
| `usuario/src/ui.rs` | Componentes de interface (botões, campos, listas, menus, diálogos) |
| `comum/cp437.rs` | A tabela da fonte da VGA e as letras acentuadas geradas no boot |
| `comum/lista_livre.rs` | Alocador usado pelo kernel e pelos programas |
| `usuario/src/lib.rs` + `sys.rs`, `io.rs` | `ulib`, a biblioteca dos programas |
| `usuario/src/bin/*.rs` | Um programa por arquivo, todos vão para `/bin` |
| `raiz/` | Arquivos que vão para o initrd (`/etc/motd`, `/home`...) |
| `disco-inicial/` | Conteúdo inicial do disco ext2 |
| `src/sinal.rs` | Sinais: ações, máscara, quadro do tratador na pilha do usuário |
| `usuario/src/usuarios.rs`, `sha256.rs` | /etc/passwd, /etc/group, /etc/shadow e o hash das senhas |
| `c/` | Programas em C, compilados com a musl |
| `dinamico/` | O carregador dinâmico (`ld-ferrugem.so`), uma biblioteca compartilhada e um programa que a usa |
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
usuário e no processo ocioso).

### Várias CPUs

No boot, o kernel lê a tabela MADT do ACPI, liga o x2APIC e acorda as outras
CPUs (INIT, SIPI, SIPI): cada uma começa em modo real no `trampolim.asm` e
sobe sozinha até o modo longo. Cada CPU tem os seus dados (GS, via `swapgs`),
a sua TSS, o seu processo ocioso e o timer do seu APIC.

O kernel inteiro é protegido por **uma trava só** (o "big kernel lock" das
primeiras versões do Linux com SMP): uma CPU a pega ao entrar no kernel e a
solta ao voltar para o programa (ou ao dormir no `hlt`). Só uma CPU por vez
roda código do kernel; os programas rodam em paralelo. E um mesmo espaço de
endereçamento (as threads de um processo) roda numa CPU por vez, então
nenhuma CPU fica com traduções velhas na TLB quando outra muda as tabelas
de páginas. O `make run` usa 4 CPUs.

## Próximos passos

1. **Portar mais programas de verdade:** o Lua 5.4 já roda sem modificações
   (`make lua`); depois, `dash` e `busybox`. O kernel avisa quando falta uma
   syscall: `chamada de sistema N não implementada`.
2. **Hardware moderno:** IOAPIC (as IRQs ainda vêm do PIC, só para a CPU
   principal), trocar a trava única do kernel por travas finas (e então
   threads do mesmo processo em paralelo, com *TLB shootdown*), boot UEFI.
3. **Rede:** driver virtio-net ou e1000, Ethernet, ARP, IP, ICMP, UDP, TCP, sockets.
4. **Interface gráfica, rumo a um desktop como o XFCE:** visualizador de
   imagens (PNG), monitor do sistema, configurações (papel de parede, cor
   de destaque); mouse absoluto no QEMU.

Boas referências: [Writing an OS in Rust](https://os.phil-opp.com/),
[OSDev Wiki](https://wiki.osdev.org/), o livro do [xv6](https://pdos.csail.mit.edu/6.828/2024/xv6/book-riscv-rev4.pdf)
(MIT), [Redox OS](https://www.redox-os.org/) e a [especificação do ext2](https://www.nongnu.org/ext2-doc/ext2.html).

## Licença

GPL-3.0-or-later (veja o arquivo `LICENSE`).
