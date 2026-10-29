#!/usr/bin/env bash
# Baixa, confere e compila a musl (a libc do Alpine Linux) em ferramentas/musl.
# Com ela, programas em C escritos para Linux compilam para o FerrugemOS.
# Compila a versão estática (libc.a) e a compartilhada (libc.so, que também é
# o carregador dinâmico /lib/ld-musl-x86_64.so.1).
set -euo pipefail
cd "$(dirname "$0")"

VERSAO=1.2.5
SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
DESTINO="$PWD/musl"

if [ -x "$DESTINO/bin/musl-gcc" ] && [ -f "$DESTINO/lib/libc.so" ]; then
    echo "musl já instalada em $DESTINO"
    exit 0
fi

mkdir -p fontes
if [ ! -f fontes/musl-$VERSAO.tar.gz ]; then
    curl -sSL -o fontes/musl-$VERSAO.tar.gz https://musl.libc.org/releases/musl-$VERSAO.tar.gz
fi
echo "$SHA256  fontes/musl-$VERSAO.tar.gz" | sha256sum -c -

rm -rf fontes/musl-$VERSAO
tar xzf fontes/musl-$VERSAO.tar.gz -C fontes
cd fontes/musl-$VERSAO
./configure --prefix="$DESTINO" > /dev/null
make -j"$(nproc)" > /dev/null
make install > /dev/null
echo "musl $VERSAO instalada em $DESTINO"
