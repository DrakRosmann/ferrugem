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

# Um servidor HTTP aqui fora, que o QEMU mostra ao FerrugemOS como
# 10.0.2.2:18080, com o arquivo que o teste da placa de rede baixa e confere.
www="$temporario/www"
mkdir -p "$www"
python3 -c 'import sys; sys.stdout.buffer.write(bytes(i * 7 % 251 for i in range(100000)))' > "$www/padrao.bin"
(cd "$www" && exec python3 -m http.server 18080 --bind 127.0.0.1) > "$temporario/http.txt" 2>&1 &
servidores=$!
# E três servidores HTTPS (TLS 1.3), com certificados de uma autoridade de
# teste feita na hora (RSA). O FerrugemOS baixa a raiz dela (ca.pem) pelo
# HTTP e confere cada cadeia:
#   18443: ChaCha20, certificado P-256 assinado pela raiz;
#   18444: AES-GCM, certificado RSA (o servidor assina com RSA-PSS) e pede o do cliente;
#   18445: certificado P-384 assinado por uma autoridade intermediária P-384 (SHA-384).
c="$temporario/certificados"
mkdir -p "$c"
(
    cd "$c"
    openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=Ferrugem Teste CA" -keyout raiz.key -out raiz.pem
    assinar() { # nome, chave da autoridade, certificado da autoridade, extensões, opções do req...
        nome=$1 ac_cert=$2 ac_chave=$3 ext=$4
        shift 4
        openssl req -new -nodes "$@" -subj "/CN=$nome" -keyout "$nome.key" -out "$nome.csr"
        openssl x509 -req -in "$nome.csr" -CA "$ac_cert" -CAkey "$ac_chave" -set_serial "$RANDOM" -days 30 -sha384 \
            -extfile <(printf '%s\n' "$ext") -out "$nome.pem"
    }
    assinar site1 raiz.pem raiz.key "subjectAltName=IP:10.0.2.2" -newkey ec -pkeyopt ec_paramgen_curve:P-256
    assinar site2 raiz.pem raiz.key "subjectAltName=IP:10.0.2.2" -newkey rsa:2048
    assinar intermediaria raiz.pem raiz.key "basicConstraints=critical,CA:TRUE" -newkey ec -pkeyopt ec_paramgen_curve:P-384
    assinar site3 intermediaria.pem intermediaria.key "subjectAltName=IP:10.0.2.2,DNS:*.ferrugem.teste" \
        -newkey ec -pkeyopt ec_paramgen_curve:P-384
    cp raiz.pem "$www/ca.pem"
    cat site3.pem intermediaria.pem > "$www/cadeia.pem"
) > "$temporario/certificados.txt" 2>&1
for tls in "18443 site1 TLS_CHACHA20_POLY1305_SHA256" "18444 site2 TLS_AES_128_GCM_SHA256 -verify 1" \
    "18445 site3 TLS_CHACHA20_POLY1305_SHA256 -cert_chain $c/intermediaria.pem"; do
    set -- $tls
    porta=$1 site=$2 cifra=$3
    shift 3
    (cd "$www" && exec openssl s_server -quiet -accept "127.0.0.1:$porta" -tls1_3 -ciphersuites "$cifra" "$@" \
        -cert "$c/$site.pem" -key "$c/$site.key" -WWW) >> "$temporario/https.txt" 2>&1 < /dev/null &
    servidores="$servidores $!"
done
trap 'kill $servidores 2>/dev/null; rm -rf "$temporario"' EXIT
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
} | timeout 70 qemu-system-x86_64 -cpu max -smp 4 -kernel build/ferrugem.bin -initrd build/initrd.tar -m 256M \
        -drive file="$disco",format=raw,if=ide,index=0 \
        -drive file="$particionado",format=raw,if=ide,index=1 \
        -nic user,model=rtl8139 \
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
} | timeout 40 qemu-system-x86_64 -cpu max -smp 4 -kernel build/ferrugem.bin -append "raiz=/dev/hda" -m 256M \
        -drive file="$sistema",format=raw,if=ide,index=0 \
        -drive file="$particionado.2",format=raw,if=ide,index=1 \
        -nic user,model=rtl8139 \
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

if grep -q "GET /padrao.bin" "$temporario/http.txt"; then
    echo "==> rede: o FerrugemOS baixou e conferiu um arquivo do servidor HTTP aqui de fora"
else
    echo "==> rede: o servidor HTTP aqui de fora não recebeu o pedido do FerrugemOS"
    falhou=1
fi

[ $falhou = 0 ] && echo "==> sucesso"
exit $falhou
