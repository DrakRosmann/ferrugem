#!/usr/bin/env bash
# Baixa, confere e compila o interpretador Lua com a musl, para o FerrugemOS.
# O código do Lua não é alterado em nada: é o mesmo que roda no Linux.
set -euo pipefail
cd "$(dirname "$0")"

VERSAO=5.4.9
# O SHA-256 publicado em https://www.lua.org/ftp/
SHA256=2335b6c582a52654f94612bf10d2f4672805d05329aa6568b1d8cd9e5c6fb8e6
MUSL_GCC="$PWD/musl/bin/musl-gcc"
DESTINO="$PWD/lua"

[ -x "$MUSL_GCC" ] || { echo "falta a musl: rode 'make musl' antes"; exit 1; }
if [ -x "$DESTINO/lua" ]; then
    echo "Lua já compilado em $DESTINO"
    exit 0
fi

mkdir -p fontes "$DESTINO"
if [ ! -f fontes/lua-$VERSAO.tar.gz ]; then
    curl -sSL -o fontes/lua-$VERSAO.tar.gz https://www.lua.org/ftp/lua-$VERSAO.tar.gz
fi
echo "$SHA256  fontes/lua-$VERSAO.tar.gz" | sha256sum -c -

rm -rf fontes/lua-$VERSAO
tar xzf fontes/lua-$VERSAO.tar.gz -C fontes
# O alvo "posix" do Lua: sem readline nem bibliotecas dinâmicas (não temos).
# Ligado em 1 GiB, onde começa a memória de usuário do FerrugemOS.
make -C fontes/lua-$VERSAO/src posix CC="$MUSL_GCC" \
    MYLDFLAGS="-static -no-pie -Wl,-Ttext-segment=0x40000000" > /dev/null
cp fontes/lua-$VERSAO/src/lua "$DESTINO/lua"
echo "Lua $VERSAO compilado em $DESTINO/lua"
