#!/usr/bin/env bash
# Inicia o FerrugemOS no QEMU. Usado como `runner` do cargo (cargo run).
#
#   scripts/qemu.sh <kernel.elf> [opções extras do QEMU]
#
# Variáveis: FERRUGEM_DISPLAY=none roda sem janela (só a serial no terminal).
set -euo pipefail

kernel="$1"
shift

# O kernel é ligado na metade alta; o carregador Multiboot recebe um binário
# plano com os endereços físicos no próprio cabeçalho.
out="$(dirname "$kernel")/ferrugem.bin"
objcopy -O binary "$kernel" "$out"

display=()
if [[ "${FERRUGEM_DISPLAY:-}" == "none" ]]; then
    display=(-display none)
fi

exec qemu-system-x86_64 \
    -kernel "$out" \
    -m 128M \
    -serial stdio \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -no-reboot \
    "${display[@]}" \
    "$@"
