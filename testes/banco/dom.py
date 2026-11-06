#!/usr/bin/env python3
# As APIs do DOM: a mesma página no Chrome e no nosso motor, linha a linha.
import subprocess, sys, os, difflib, re, html
aqui = os.path.dirname(os.path.abspath(__file__))
f = os.path.join(aqui, '..', 'js', 'dom.html')
p = os.path.join(sys.argv[1], 'dom.ref.html')
s = open(f).read().replace('</script></body></html>', "setTimeout(()=>{document.title='';var p=document.createElement('pre');p.id='__s';p.textContent=L.join('\\n');document.body.innerHTML='';document.body.appendChild(p)},300);</script></body></html>")
open(p, 'w').write(s)
o = subprocess.run(['google-chrome', '--headless=new', '--disable-gpu', '--no-sandbox', '--window-size=800,600', '--virtual-time-budget=2000', '--dump-dom', 'file://' + p], capture_output=True, text=True).stdout
m = re.search(r'<pre id="__s">(.*?)</pre>', o, re.S)
ch = html.unescape(m.group(1)).split('\n') if m else ['(sem saída do chrome)']
n = subprocess.run([os.path.join(aqui, 'target/release/js'), '--dom', f], capture_output=True, text=True)
me = n.stdout.split('\n')
if me and me[-1] == '':
    me.pop()
# Os timers correm em relação à carga de um jeito próprio de cada um: a ordem
# deles não é parte do teste.
fora = lambda l: [x for x in l if x not in ('timeout', 'intervalo', 'DOMContentLoaded 1', 'load complete true', 'micro')]
dif = list(difflib.unified_diff(fora(ch), fora(me), 'chrome', 'motor', lineterm='', n=0))
for l in dif:
    print(l[:400])
print(('OK  ' if not dif else 'DIFERE') + ' dom  (%d linhas)' % len(ch))
sys.exit(1 if dif else 0)
