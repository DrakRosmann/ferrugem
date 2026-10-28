#!/usr/bin/env bash
# Cria a imagem de disco do FerrugemOS: um ext2 com /bin (os programas de
# user/), /etc, /home (de sistema/) e os pontos de montagem /dev e /tmp.
#
#   scripts/disco.sh [imagem]      (padrão: target/disco.img)
#
# Se a imagem já existe, o /home dela é preservado: só o sistema é refeito.
# Variáveis: FERRUGEM_DISCO_MB (tamanho, padrão 64), FERRUGEM_DISCO_NOVO=1
# (não preserva nada), FERRUGEM_BLOCO (tamanho do bloco: 1024, 2048, 4096).
# Não precisa de root: o mkfs.ext2 -d copia os arquivos para dentro da imagem.
set -euo pipefail

raiz="$(cd "$(dirname "$0")/.." && pwd)"
imagem="${1:-$raiz/target/disco.img}"
programas="$raiz/target/programas"
if [[ ! -d "$programas" ]]; then
    echo "disco.sh: compile antes (cargo build --release)" >&2
    exit 1
fi

pasta="$(mktemp -d)"
trap 'rm -rf "$pasta"' EXIT
cp -r "$raiz/sistema/." "$pasta/"
mkdir -p "$pasta"/{bin,dev,tmp,mnt}
cp "$programas"/* "$pasta/bin/"
chmod 755 "$pasta"/bin/*

if [[ -f "$imagem" && "${FERRUGEM_DISCO_NOVO:-}" != 1 ]]; then
    rm -rf "$pasta/home"
    debugfs -R "rdump /home $pasta" "$imagem" 2>/dev/null
    echo "disco.sh: /home preservado de $imagem"
fi

rm -f "$imagem.novo"
mkfs.ext2 -q -F -L ferrugem -b "${FERRUGEM_BLOCO:-1024}" -O ^dir_index -E root_owner=0:0 \
    -d "$pasta" "$imagem.novo" "${FERRUGEM_DISCO_MB:-64}M"
mv "$imagem.novo" "$imagem"
echo "disco.sh: $imagem pronto"
