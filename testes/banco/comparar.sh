#!/bin/bash
# Compara os motores do navegador com os de verdade, rodando no Linux:
#   testes/js/*.js   com o node (a saída tem que ser igual);
#   testes/css/*.html com o Chrome (a caixa de cada elemento, com 1 pixel de folga);
#   testes/js/dom.html com o Chrome (as APIs do DOM).
# Precisa do node e do google-chrome. Uso: testes/banco/comparar.sh
cd "$(dirname "$0")" || exit 1
B=$PWD; T=$B/..; rc=0
# O `Url` do http.rs, sem o resto (a rede só existe dentro do FerrugemOS).
python3 - <<'PY'
s = open('../../usuario/src/http.rs').read()
fim = s.rfind('\n\n', 0, s.index('pub struct Resposta'))
open('src/http.rs', 'w').write('\n'.join(l for l in s[:fim].split('\n') if not l.startswith('use crate::')) + '\n')
PY
cargo build --release 2>&1 | grep -E '^error' -A7
SAIDA=$(mktemp -d); trap 'rm -rf "$SAIDA"' EXIT
for f in "$T"/js/*.js; do
  n=$(basename "$f" .js)
  LC_ALL=C LANG=C TZ=UTC node "$f" > "$SAIDA/$n.node" 2>&1
  TZ=UTC timeout 60 target/release/js "$f" > "$SAIDA/$n.nosso" 2>&1
  if diff -q "$SAIDA/$n.node" "$SAIDA/$n.nosso" > /dev/null; then echo "OK   js  $n"; else echo "DIFERE js $n"; diff "$SAIDA/$n.node" "$SAIDA/$n.nosso" | head -30; rc=1; fi
done
W=800
for f in "$T"/css/*.html; do
  n=$(basename "$f" .html); P=$SAIDA/$n.html
  cp "$f" "$P"
  cat >> "$P" <<'JS'
<script>var o=[];document.querySelectorAll('[id]').forEach(e=>{var r=e.getBoundingClientRect();o.push(e.id+' '+Math.floor(r.x)+' '+Math.floor(r.y)+' '+Math.floor(r.width)+' '+Math.floor(r.height))});document.title='';document.body.innerHTML='<pre id=__s>'+o.join('\n')+'</pre>'</script>
JS
  google-chrome --headless=new --disable-gpu --no-sandbox --window-size=$W,600 --hide-scrollbars --dump-dom "file://$P" 2>/dev/null | sed -n '/<pre id="__s">/,/<\/pre>/p' | sed 's/<pre id="__s">//; s/<\/pre>.*//' > "$SAIDA/$n.chrome"
  target/release/js --layout "$f" $W > "$SAIDA/$n.motor"
  if python3 compara.py "$SAIDA/$n.chrome" "$SAIDA/$n.motor" > "$SAIDA/$n.dif"; then echo "OK   css $n"; else echo "DIFERE css $n"; head -40 "$SAIDA/$n.dif"; rc=1; fi
done
python3 dom.py "$SAIDA" || rc=1
exit $rc
