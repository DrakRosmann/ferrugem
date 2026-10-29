#!/usr/bin/env bash
# Boota o FerrugemOS sem janela, digita comandos pela serial e confere a saída.
# Usa um disco descartável (não mexe no seu disco.img) e, no fim, confere o
# ext2 que o kernel gravou com o e2fsck do Linux.
# Uso: ./testes/rodar.sh   (ou `make teste`)
set -u
cd "$(dirname "$0")/.."

temporario=$(mktemp -d)
trap 'rm -rf "$temporario"' EXIT
saida="$temporario/saida.txt"
disco="$temporario/disco.img"
# Os testes da libc (c/testec.c) só existem se a musl estiver instalada (`make musl`).
comandos="teste"
if [ -f build/raiz/bin/testec ]; then
    comandos="teste testec"
else
    echo "(sem a musl: pulando os testes da libc; rode 'make musl' para incluí-los)"
fi

mke2fs -q -t ext2 -b 1024 -I 128 -O none,filetype,sparse_super -E root_owner=1000:1000 -d disco-inicial "$disco" 32M >/dev/null 2>&1

{
    sleep 2
    # login como root (os testes de permissão começam como root e viram o usuário 1000)
    printf 'root\r'; sleep 0.5; printf 'root\r'; sleep 1.5
    for comando in $comandos desligar; do
        printf '%s\r' "$comando"
        sleep 10
    done
} | timeout 70 qemu-system-x86_64 -cpu max -kernel build/ferrugem.bin -initrd build/initrd.tar -m 256M \
        -drive file="$disco",format=raw,if=ide,index=0 \
        -display none -chardev stdio,id=s0,signal=off -serial chardev:s0 -no-reboot > "$saida" 2>&1

sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$saida" | sed -n '/Rodando/,/testes /p;/Testando a libc/,/testes da libc/p'

falhou=0
libc_ok=true
[[ $comandos == *testec* ]] && ! grep -q "testes da libc passaram" "$saida" && libc_ok=false
if ! grep -q "testes passaram" "$saida" || [ $libc_ok = false ]; then
    echo "==> os testes FALHARAM (saída completa abaixo)"
    sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$saida" | tail -40
    falhou=1
fi
if e2fsck -fn "$disco" > "$temporario/fsck.txt" 2>&1; then
    echo "==> e2fsck: o ext2 gravado pelo kernel está consistente"
else
    echo "==> e2fsck encontrou erros no disco:"
    cat "$temporario/fsck.txt"
    falhou=1
fi
[ $falhou = 0 ] && echo "==> sucesso"
exit $falhou
