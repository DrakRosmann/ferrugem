KERNEL := target/x86_64-unknown-none/release/ferrugem
ISO    := target/ferrugem.iso

.PHONY: build run run-serial iso run-iso clean

build:
	cargo build --release

# Janela do QEMU com a tela VGA; a saída serial aparece no terminal.
run: build
	scripts/qemu.sh $(KERNEL)

# Sem janela: use o shell pelo próprio terminal.
run-serial: build
	FERRUGEM_DISPLAY=none scripts/qemu.sh $(KERNEL)

# Imagem inicializável com GRUB (para pendrive ou outras VMs).
iso: build
	rm -rf target/iso
	mkdir -p target/iso/boot/grub
	objcopy -O binary $(KERNEL) target/iso/boot/ferrugem.bin
	printf 'set timeout=0\nmenuentry "FerrugemOS" {\n  multiboot /boot/ferrugem.bin\n  boot\n}\n' \
		> target/iso/boot/grub/grub.cfg
	grub2-mkrescue -o $(ISO) target/iso

run-iso: iso
	qemu-system-x86_64 -cdrom $(ISO) -m 128M -serial stdio

clean:
	cargo clean
