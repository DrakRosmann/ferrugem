# =============================================================================
#  FerrugemOS — build
#
#    make          compila o kernel e os programas -> build/ferrugem.bin + build/initrd.tar
#    make run      roda no QEMU (janela + terminal na serial)
#    make run-nox  roda só no terminal (sem janela; Ctrl+A depois X para sair)
#    make teste    boota, roda a bateria de testes (/bin/teste) e desliga
#    make disco    recria o disco ext2 (disco.img) do zero
#    make run-sistema  boota com a raiz no disco (sistema.img: o sistema inteiro num ext2)
#    make sistema  recria o sistema.img do zero
#    make musl     baixa e compila a musl (libc) para os programas em C de c/
#    make lua      baixa e compila o Lua 5.4 (sem modificações) para /bin/lua
#    make clean
# =============================================================================

# Alvo "bare metal" do Rust (sem sistema operacional por baixo).
# Fedora: sudo dnf install rust-std-static-x86_64-unknown-none
# rustup: rustup target add x86_64-unknown-none
TARGET ?= x86_64-unknown-none

BUILD   := build
KERNEL  := $(BUILD)/ferrugem.bin
ELF64   := $(BUILD)/kernel.elf
BOOT_O  := $(BUILD)/boot.o
ISR_O   := $(BUILD)/interrupcoes.o
RUSTLIB := target/$(TARGET)/release/libferrugem.a

INITRD    := $(BUILD)/initrd.tar
# Fora de build/ de propósito: o `make clean` não apaga o que você gravou no disco.
DISCO     := disco.img
# O sistema inteiro num disco ext2, para bootar com a raiz no disco (make run-sistema).
SISTEMA   := sistema.img
RAIZ      := $(BUILD)/raiz
USUARIO_OK := $(BUILD)/usuario.ok
USUARIO_BIN := usuario/target/$(TARGET)/release
PROGRAMAS := $(basename $(notdir $(wildcard usuario/src/bin/*.rs)))

# Programas em C, compilados com a musl (`make musl` instala). Ligados em 1 GiB,
# onde começa a memória de usuário do FerrugemOS (o padrão do Linux é 4 MiB).
MUSL_GCC   := ferramentas/musl/bin/musl-gcc
PROGRAMAS_C := $(if $(wildcard $(MUSL_GCC)),$(basename $(notdir $(wildcard c/*.c))))
BINARIOS_C := $(addprefix $(BUILD)/c/,$(PROGRAMAS_C))
FLAGS_C    := -static -no-pie -O2 -Wall -Wl,-Ttext-segment=0x40000000
# Bibliotecas compartilhadas: o nosso carregador dinâmico (/lib/ld-ferrugem.so),
# uma biblioteca de exemplo e um programa que a usa. Em C sem libc, com o
# compilador do sistema (veja dinamico/).
DINAMICO     := $(BUILD)/dinamico
FLAGS_DIN    := -O2 -Wall -ffreestanding -fno-stack-protector -fno-builtin \
                -fno-tree-loop-distribute-patterns -nostdlib -Wl,--hash-style=sysv -Wl,-z,noexecstack
BINARIOS_DIN := $(DINAMICO)/ld-ferrugem.so $(DINAMICO)/libsaudacao.so $(DINAMICO)/dinamico
# Com a musl compilada também como biblioteca compartilhada (o ferramentas/musl.sh
# atual faz isso), a libc.so dela vira /lib/ld-musl-x86_64.so.1 (ela é o próprio
# carregador) e o c/ola.c ganha uma versão dinâmica, /bin/ola-dinamico.
MUSL_SO      := $(wildcard ferramentas/musl/lib/libc.so)
BINARIOS_DIN += $(if $(MUSL_SO),$(BUILD)/c/ola-dinamico)
# Fontes TrueType para a interface gráfica: as que o sistema de quem compila
# tiver (não entram no repositório). Sem elas, tudo usa a fonte da VGA.
FONTES_TT := $(wildcard /usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf \
                        /usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf \
                        /usr/share/fonts/liberation-mono-fonts/LiberationMono-Regular.ttf)
# Programas de terceiros, sem nenhuma modificação (`make lua`).
LUA        := $(wildcard ferramentas/lua/lua)

# Kernel: código "estático" (sem relocação dinâmica), pois não existe loader de ELF dinâmico.
FLAGS_KERNEL := -C relocation-model=static -C code-model=small
# Programas: o mesmo, ligados pelo ld.lld no endereço 1 GiB (veja usuario/usuario.ld).
FLAGS_USUARIO := -C linker=ld.lld -C relocation-model=static -C link-arg=-T$(CURDIR)/usuario/usuario.ld

RUST_SRC    := $(wildcard src/*.rs) $(wildcard comum/*.rs) Cargo.toml
USUARIO_SRC := $(wildcard usuario/src/*.rs usuario/src/bin/*.rs) $(wildcard comum/*.rs) \
               usuario/Cargo.toml usuario/usuario.ld
ARQUIVOS_RAIZ := $(shell find raiz -type f)

# 4 CPUs (o kernel acorda as outras três: veja src/smp.rs).
QEMU := qemu-system-x86_64 -cpu max -smp 4 -kernel $(KERNEL) -initrd $(INITRD) -m 256M \
        -drive file=$(DISCO),format=raw,if=ide,index=0

.PHONY: all run run-nox run-sistema sistema teste clean disco musl lua

all: $(KERNEL) $(INITRD) $(DISCO)

$(BUILD):
	mkdir -p $(BUILD)

$(BOOT_O): boot/boot.asm | $(BUILD)
	nasm -f elf64 $< -o $@

$(ISR_O): src/interrupcoes.asm | $(BUILD)
	nasm -f elf64 $< -o $@

# O código onde as outras CPUs começam (16 bits): binário puro, embutido no kernel.
TRAMPOLIM := $(BUILD)/trampolim.bin
$(TRAMPOLIM): src/trampolim.asm | $(BUILD)
	nasm -f bin $< -o $@

$(RUSTLIB): $(RUST_SRC) $(TRAMPOLIM)
	RUSTFLAGS="$(FLAGS_KERNEL)" cargo build --release --target $(TARGET)

# 1) liga boot.o + interrupções + Rust num ELF de 64 bits
# 2) converte para ELF32, que é o formato que o carregador Multiboot do QEMU aceita
$(KERNEL): $(BOOT_O) $(ISR_O) $(RUSTLIB) linker.ld
	ld -n -m elf_x86_64 --gc-sections -z noexecstack --no-warn-rwx-segments -T linker.ld -o $(ELF64) $(BOOT_O) $(ISR_O) $(RUSTLIB)
	objcopy -O elf32-i386 $(ELF64) $@
	@echo "==> $@ pronto"

$(USUARIO_OK): $(USUARIO_SRC) | $(BUILD)
	cd usuario && RUSTFLAGS="$(FLAGS_USUARIO)" cargo build --release --target $(TARGET)
	touch $@

musl:
	./ferramentas/musl.sh

# Baixa o Lua 5.4.9 de lua.org, confere o SHA-256, compila com a musl e põe em /bin/lua.
lua: musl
	./ferramentas/lua.sh
	rm -f $(INITRD)
	$(MAKE) $(INITRD)

$(BUILD)/c/%: c/%.c $(MUSL_GCC) | $(BUILD)
	mkdir -p $(BUILD)/c
	$(MUSL_GCC) $(FLAGS_C) -o $@ $<

# O carregador não pode ter relocações (ninguém o reloca): tudo "hidden".
$(DINAMICO)/ld-ferrugem.so: dinamico/ld-ferrugem.c dinamico/sys.h | $(BUILD)
	mkdir -p $(DINAMICO)
	$(CC) $(FLAGS_DIN) -fPIC -fvisibility=hidden -shared -Wl,-e,_start -Wl,-Bsymbolic -o $@ $<
$(DINAMICO)/libsaudacao.so: dinamico/libsaudacao.c dinamico/sys.h | $(BUILD)
	mkdir -p $(DINAMICO)
	$(CC) $(FLAGS_DIN) -fPIC -shared -Wl,-soname,libsaudacao.so -o $@ $<
# PIE: vai para uma base sorteada (as specs do musl-gcc não repassam -pie ao ld).
$(BUILD)/c/ola-dinamico: c/ola.c $(MUSL_SO) | $(BUILD)
	mkdir -p $(BUILD)/c
	$(MUSL_GCC) -O2 -Wall -fPIE -Wl,-pie -o $@ $<
$(DINAMICO)/dinamico: dinamico/dinamico.c dinamico/sys.h $(DINAMICO)/libsaudacao.so
	$(CC) $(FLAGS_DIN) -fPIE -pie -Wl,-z,now -Wl,--dynamic-linker=/lib/ld-ferrugem.so \
		-L$(DINAMICO) -o $@ $< -lsaudacao

# O initrd é um tar com a árvore de arquivos inicial: raiz/ + os programas em /bin
$(INITRD): $(USUARIO_OK) $(ARQUIVOS_RAIZ) $(BINARIOS_C) $(BINARIOS_DIN) $(LUA)
	@$(if $(PROGRAMAS_C),,echo "(sem a musl: os programas em c/ ficam de fora; rode 'make musl')")
	rm -rf $(RAIZ)
	mkdir -p $(RAIZ)/bin
	cp -r raiz/. $(RAIZ)/
	for p in $(PROGRAMAS); do strip -o $(RAIZ)/bin/$$p $(USUARIO_BIN)/$$p; done
	for p in $(PROGRAMAS_C); do strip -o $(RAIZ)/bin/$$p $(BUILD)/c/$$p; done
	$(if $(LUA),strip -o $(RAIZ)/bin/lua $(LUA))
	mkdir -p $(RAIZ)/lib $(RAIZ)/var/log $(RAIZ)/var/spool/cron
	cp $(DINAMICO)/ld-ferrugem.so $(DINAMICO)/libsaudacao.so $(RAIZ)/lib/
	cp $(DINAMICO)/dinamico $(RAIZ)/bin/
	$(if $(FONTES_TT),mkdir -p $(RAIZ)/usr/share/fonts && cp $(FONTES_TT) $(RAIZ)/usr/share/fonts/)
	$(if $(MUSL_SO),cp $(MUSL_SO) $(RAIZ)/lib/ld-musl-x86_64.so.1 && cp $(BUILD)/c/ola-dinamico $(RAIZ)/bin/)
	chmod 4755 $(RAIZ)/bin/su $(RAIZ)/bin/passwd $(RAIZ)/bin/crontab    # setuid root
	chmod 700 $(RAIZ)/var/spool/cron
	chmod 600 $(RAIZ)/etc/shadow
	chmod 700 $(RAIZ)/root
	# Tudo é do root, menos o /home/usuario, que é do usuário 1000.
	tar --format=ustar --numeric-owner --owner=0 --group=0 --exclude=./home/usuario -cf $@ -C $(RAIZ) .
	tar --format=ustar --numeric-owner --owner=1000 --group=1000 -rf $@ -C $(RAIZ) ./home/usuario
	@echo "==> $@ pronto ($(words $(PROGRAMAS)) programas em Rust, $(words $(PROGRAMAS_C)) em C)"

# Disco de 32 MiB com ext2 (blocos de 1 KiB, sem recursos modernos que o
# kernel não sabe gravar), já com o conteúdo de disco-inicial/.
$(DISCO):
	mke2fs -q -t ext2 -b 1024 -I 128 -O none,filetype,sparse_super -E root_owner=1000:1000 -L ferrugem -d disco-inicial $@ 32M >/dev/null 2>&1
	@echo "==> $@ criado"

# Recria o disco do zero (apaga o que foi gravado nele!)
disco:
	rm -f $(DISCO)
	$(MAKE) $(DISCO)

# Criado uma vez (depois, o que você mudar nele fica). `make sistema` recria do zero.
$(SISTEMA): | $(INITRD)
	./ferramentas/sistema.sh $@

sistema: $(INITRD)
	./ferramentas/sistema.sh $(SISTEMA)

# Raiz no disco: sem initrd; o kernel monta o hda como "/" (raiz=/dev/hda).
run-sistema: $(KERNEL) $(SISTEMA)
	qemu-system-x86_64 -cpu max -smp 4 -kernel $(KERNEL) -append "raiz=/dev/hda" -m 256M \
		-drive file=$(SISTEMA),format=raw,if=ide,index=0 \
		-chardev stdio,id=serial0,signal=off -serial chardev:serial0

# signal=off: o Ctrl+C vai para o FerrugemOS em vez de matar o QEMU.
run: all
	$(QEMU) -chardev stdio,id=serial0,signal=off -serial chardev:serial0

run-nox: all
	$(QEMU) -display none -chardev stdio,id=serial0,mux=on,signal=off -serial chardev:serial0 -mon chardev=serial0

teste: all
	./testes/rodar.sh

clean:
	cargo clean
	cd usuario && cargo clean
	rm -rf $(BUILD)
