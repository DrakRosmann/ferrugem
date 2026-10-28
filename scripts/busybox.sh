#!/usr/bin/env bash
# Compila o busybox (centenas de comandos Unix num executável só) como
# binário estático com a musl, para rodar no FerrugemOS.
#
#   scripts/busybox.sh       →  target/busybox/busybox
#
# Não precisa de root nem de pacotes da musl: a própria musl é compilada
# para dentro de target/musl. Os fontes são baixados uma vez (para
# target/externos) e conferidos pelo SHA-256. Depois, `make disco` põe o
# busybox no /bin do disco e os comandos dele em /usr/bin.
set -euo pipefail

MUSL=musl-1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
BUSYBOX=busybox-1.37.0
BUSYBOX_SHA256=3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4

raiz="$(cd "$(dirname "$0")/.." && pwd)"
externos="$raiz/target/externos"
musl="$raiz/target/musl"
saida="$raiz/target/busybox"
nucleos="$(nproc 2>/dev/null || echo 2)"
mkdir -p "$externos" "$saida"

baixar() { # url arquivo sha256
    local arquivo="$externos/$2"
    if [[ ! -f "$arquivo" ]]; then
        echo "busybox.sh: baixando $2"
        curl -sSfL -o "$arquivo.parcial" "$1"
        mv "$arquivo.parcial" "$arquivo"
    fi
    echo "$3  $arquivo" | sha256sum -c --quiet
}

baixar "https://musl.libc.org/releases/$MUSL.tar.gz" "$MUSL.tar.gz" "$MUSL_SHA256"
baixar "https://busybox.net/downloads/$BUSYBOX.tar.bz2" "$BUSYBOX.tar.bz2" "$BUSYBOX_SHA256"

# 1. A musl, instalada em target/musl (com o script musl-gcc).
if [[ ! -x "$musl/bin/musl-gcc" ]]; then
    echo "busybox.sh: compilando a $MUSL"
    rm -rf "$externos/$MUSL"
    tar -xzf "$externos/$MUSL.tar.gz" -C "$externos"
    (cd "$externos/$MUSL" && ./configure --prefix="$musl" --disable-shared >/dev/null && make -s -j"$nucleos" && make -s install) >/dev/null
    # O busybox inclui cabeçalhos do kernel Linux (linux/, asm/), que a musl
    # não traz: copia os do sistema.
    for dir in linux asm asm-generic mtd; do
        for origem in "/usr/include/$dir" "/usr/include/x86_64-linux-gnu/$dir"; do
            if [[ -d "$origem" && ! -e "$musl/include/$dir" ]]; then
                cp -r "$origem" "$musl/include/$dir"
            fi
        done
    done
fi

# 2. O busybox, com a configuração padrão, estático.
fonte="$externos/$BUSYBOX"
if [[ ! -d "$fonte" ]]; then
    tar -xjf "$externos/$BUSYBOX.tar.bz2" -C "$externos"
fi
echo "busybox.sh: compilando o $BUSYBOX"
cd "$fonte"
make -s CC="$musl/bin/musl-gcc" defconfig >/dev/null
desligar() { sed -i "s/^$1=y/# $1 is not set/" .config; }
ligar() { sed -i "s/^# $1 is not set/$1=y/" .config; }
ligar CONFIG_STATIC
# Não compilam com a musl ou com cabeçalhos novos do kernel.
desligar CONFIG_TC
desligar CONFIG_FEATURE_TC_INGRESS
# `uname -o` mostra o nome do sistema.
sed -i 's/^CONFIG_UNAME_OSNAME=.*/CONFIG_UNAME_OSNAME="FerrugemOS"/' .config
make -s CC="$musl/bin/musl-gcc" oldconfig >/dev/null </dev/null
# Os avisos do compilador (do código do busybox) vão para um log.
make -s -j"$nucleos" CC="$musl/bin/musl-gcc" busybox >/dev/null 2>"$saida/compilacao.log"
cp busybox "$saida/busybox"
"$saida/busybox" --list-full > "$saida/comandos"
echo "busybox.sh: $saida/busybox pronto ($(wc -l < "$saida/comandos") comandos)"
