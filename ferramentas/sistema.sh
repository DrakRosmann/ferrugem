#!/usr/bin/env bash
# Monta um disco ext2 com o FerrugemOS inteiro, para bootar com a raiz no
# disco (raiz=/dev/hda). Usa a árvore que o `make` prepara em build/raiz.
# Uso: ./ferramentas/sistema.sh <imagem>
set -euo pipefail
cd "$(dirname "$0")/.."
destino=${1:-sistema.img}
[ -d build/raiz/bin ] || { echo "falta build/raiz: rode 'make' antes"; exit 1; }

arvore=$(mktemp -d)
comandos=$(mktemp)
trap 'rm -rf "$arvore" "$comandos"' EXIT
cp -a build/raiz/. "$arvore/"
mkdir -p "$arvore"/{dev,proc,tmp,mnt,disco}   # pontos de montagem
chmod 1777 "$arvore/tmp"

rm -f "$destino"
mke2fs -q -t ext2 -b 1024 -I 128 -O none,filetype,sparse_super -E root_owner=0:0 \
    -L ferrugem -d "$arvore" "$destino" 64M >/dev/null 2>&1

# O mke2fs copia o dono que os arquivos têm aqui (você). No sistema, tudo é
# do root, menos o /home/usuario (uid 1000).
(cd "$arvore" && find . -mindepth 1 | sed 's|^\.||') | while read -r caminho; do
    case "$caminho" in
        /home/usuario | /home/usuario/*) dono=1000 ;;
        *) dono=0 ;;
    esac
    echo "sif \"$caminho\" uid $dono"
    echo "sif \"$caminho\" gid $dono"
done > "$comandos"
debugfs -w -f "$comandos" "$destino" >/dev/null 2>&1
echo "==> $destino pronto (o sistema inteiro num ext2)"
