KERNEL := target/x86_64-unknown-none/release/ferrugem
ISO    := target/ferrugem.iso
DISCO  := target/disco.img

.PHONY: build run run-serial test disco iso run-iso clean

build:
	cargo build --release

# Janela do QEMU com a tela VGA; a saída serial aparece no terminal.
run: build $(DISCO)
	scripts/qemu.sh $(KERNEL)

# Sem janela: use o shell pelo próprio terminal.
run-serial: build $(DISCO)
	FERRUGEM_DISPLAY=none scripts/qemu.sh $(KERNEL)

# O disco só é criado se não existir, para não perder o que foi gravado nele.
$(DISCO):
	$(MAKE) build
	scripts/disco.sh $(DISCO)

# Refaz o disco com os programas atuais, preservando o /home.
disco: build
	scripts/disco.sh $(DISCO)

# Testes de integração: cada tests/*.txt é uma sessão no sh, no QEMU.
test: build
	python3 scripts/testar.py

# Imagem inicializável com GRUB (para pendrive ou outras VMs).
iso: build
	rm -rf target/iso
	mkdir -p target/iso/boot/grub
	objcopy -O binary $(KERNEL) target/iso/boot/ferrugem.bin
	printf 'set timeout=0\nmenuentry "FerrugemOS" {\n  multiboot /boot/ferrugem.bin\n  boot\n}\n' \
		> target/iso/boot/grub/grub.cfg
	grub2-mkrescue -o $(ISO) target/iso

run-iso: iso $(DISCO)
	qemu-system-x86_64 -cdrom $(ISO) -m 128M -serial stdio -drive file=$(DISCO),format=raw,if=ide

clean:
	cargo clean
