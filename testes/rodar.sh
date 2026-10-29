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
# O Lua (de lua.org, sem modificações), se estiver instalado (`make lua`).
if [ -f build/raiz/bin/lua ]; then
    comandos="$comandos lua_teste"
fi

mke2fs -q -t ext2 -b 1024 -I 128 -O none,filetype,sparse_super -E root_owner=1000:1000 -d disco-inicial "$disco" 32M >/dev/null 2>&1
# Um segundo disco (hdb) com tabela de partições MBR e um ext2 na partição 1
# (que começa no setor 2048, a 1 MiB do início).
particionado="$temporario/particionado.img"
truncate -s 24M "$particionado"
echo 'start=2048, size=30720, type=83' | sfdisk -q "$particionado" >/dev/null 2>&1
mke2fs -q -t ext2 -b 1024 -I 128 -O none,filetype,sparse_super -E offset=1048576 "$particionado" 15M >/dev/null 2>&1

{
    sleep 2
    # login como root (os testes de permissão começam como root e viram o usuário 1000)
    printf 'root\r'; sleep 0.5; printf 'root\r'; sleep 1.5
    for comando in $comandos desligar; do
        if [ "$comando" = lua_teste ]; then
            printf '%s\r' "lua -e 'local t = {} for i = 1, 5 do t[i] = i * i end print(\"lua:\", table.concat(t, \",\"), #io.popen(\"ls /bin\"):read(\"a\") > 0)'"
            sleep 3
            continue
        fi
        printf '%s\r' "$comando"
        sleep 10
    done
} | timeout 70 qemu-system-x86_64 -cpu max -kernel build/ferrugem.bin -initrd build/initrd.tar -m 256M \
        -drive file="$disco",format=raw,if=ide,index=0 \
        -drive file="$particionado",format=raw,if=ide,index=1 \
        -display none -chardev stdio,id=s0,signal=off -serial chardev:s0 -no-reboot > "$saida" 2>&1

sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$saida" | sed -n '/Rodando/,/testes /p;/Testando a libc/,/testes da libc/p'

falhou=0
libc_ok=true
[[ $comandos == *testec* ]] && ! grep -q "testes da libc passaram" "$saida" && libc_ok=false
if [[ $comandos == *lua_teste* ]]; then
    if grep -q "lua:	1,4,9,16,25	true" "$saida"; then
        echo "  [ OK ] Lua 5.4 (sem modificações): tabelas, strings e io.popen"
    else
        echo "  [FALHOU] Lua"
        libc_ok=false
    fi
fi
if ! grep -q "testes passaram" "$saida" || [ $libc_ok = false ]; then
    echo "==> os testes FALHARAM (saída completa abaixo)"
    sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$saida" | tail -40
    falhou=1
fi
dd if="$particionado" of="$temporario/particao.img" bs=1M skip=1 count=15 status=none
if e2fsck -fn "$disco" > "$temporario/fsck.txt" 2>&1 && e2fsck -fn "$temporario/particao.img" >> "$temporario/fsck.txt" 2>&1; then
    echo "==> e2fsck: os dois ext2 gravados pelo kernel (hda e hdb1) estão consistentes"
else
    echo "==> e2fsck encontrou erros no disco:"
    cat "$temporario/fsck.txt"
    falhou=1
fi
# ---------------------------------------------------------------------------
# Segunda rodada: a mesma bateria do kernel, agora com a raiz no disco
# (raiz=/dev/hda, sem initrd): programas, /etc e /home vêm de um ext2.
# ---------------------------------------------------------------------------
echo
echo "Com a raiz no disco (raiz=/dev/hda):"
sistema="$temporario/sistema.img"
./ferramentas/sistema.sh "$sistema" > /dev/null
truncate -s 24M "$particionado.2"
echo 'start=2048, size=30720, type=83' | sfdisk -q "$particionado.2" >/dev/null 2>&1
mke2fs -q -t ext2 -b 1024 -I 128 -O none,filetype,sparse_super -E offset=1048576 "$particionado.2" 15M >/dev/null 2>&1
{
    sleep 2
    printf 'root\r'; sleep 0.5; printf 'root\r'; sleep 1.5
    printf 'teste\r'; sleep 12
    printf 'desligar\r'; sleep 3
} | timeout 40 qemu-system-x86_64 -cpu max -kernel build/ferrugem.bin -append "raiz=/dev/hda" -m 256M \
        -drive file="$sistema",format=raw,if=ide,index=0 \
        -drive file="$particionado.2",format=raw,if=ide,index=1 \
        -display none -chardev stdio,id=s0,signal=off -serial chardev:s0 -no-reboot > "$saida.2" 2>&1
sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$saida.2" | grep -E "raiz no disco|FALHOU|testes (passaram|falharam)" | sed 's/^/  /'
if ! grep -q "testes passaram" "$saida.2"; then
    echo "==> com a raiz no disco, os testes FALHARAM (fim da saída abaixo)"
    sed 's/\x1b\[[0-9;]*[a-zA-Z]//g' "$saida.2" | tail -30
    falhou=1
elif e2fsck -fn "$sistema" > "$temporario/fsck2.txt" 2>&1; then
    echo "==> e2fsck: o disco do sistema (a raiz) está consistente"
else
    echo "==> e2fsck encontrou erros no disco do sistema:"
    cat "$temporario/fsck2.txt"
    falhou=1
fi

[ $falhou = 0 ] && echo "==> sucesso"
exit $falhou
