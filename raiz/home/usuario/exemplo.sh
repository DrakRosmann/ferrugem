# Um script de shell: rode com `sh /home/exemplo.sh`
echo "== quem sou eu"
uname -a
echo "== criando arquivos em /tmp"
mkdir -p /tmp/exemplo
echo "primeira linha" > /tmp/exemplo/texto.txt
echo "segunda linha" >> /tmp/exemplo/texto.txt
cat /tmp/exemplo/texto.txt | wc -l
echo "== processos"
ps
