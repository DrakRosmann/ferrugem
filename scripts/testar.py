#!/usr/bin/env python3
"""Testes de integração do FerrugemOS.

Cada arquivo em tests/*.txt é uma sessão no /bin/sh: o sistema dá boot no
QEMU (sem janela, pela serial), e cada comando só é digitado quando o prompt
aparece. A saída de cada comando é comparada com a esperada.

Formato dos testes:

    # comentário (linhas começando com #)
    $ echo oi | cat         comando digitado no sh
    oi                      saída esperada (linhas seguintes)
    $ yes > /dev/null
    <Ctrl+Z>                tecla enviada enquanto o comando roda
    ^Z
    [1]  Parado      yes >/dev/null

Nas linhas esperadas, `*` casa com qualquer trecho (útil para pids e tempos).
Espaços no fim das linhas e as cores (sequências ANSI) são ignorados.

Cada arquivo roda com uma cópia nova do disco de teste (um ext2 criado por
scripts/disco.sh). No fim, o sistema é desligado e o `e2fsck` verifica o
disco: qualquer inconsistência deixada pelo ext2 do kernel é uma falha.
Diretivas:

    @reiniciar     desliga e dá boot de novo no mesmo disco
    @sem-disco     (no início) roda sem disco, com a raiz em memória

Uso: scripts/testar.py [--gravar] [tests/arquivo.txt...]
  --gravar  só mostra a saída real de cada comando (para escrever testes)
"""

import codecs
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

RAIZ = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KERNEL = os.path.join(RAIZ, "target/x86_64-unknown-none/release/ferrugem")
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
PROMPT = re.compile(r"ferrugem:[^\n]*\$ $")
MONITOR = re.compile(r"ferrugem> $")
DISCO_TESTE = os.path.join(RAIZ, "target/teste-disco.img")
PANICO = "PÂNICO DO KERNEL"
TECLAS = {"<Ctrl+C>": "\x03", "<Ctrl+Z>": "\x1a", "<Ctrl+D>": "\x04", "<Ctrl+\\>": "\x1c"}
TEMPO_BOOT = 60
TEMPO_COMANDO = 30


class Falha(Exception):
    pass


class Maquina:
    """O QEMU rodando o kernel, com a serial ligada a este processo."""

    def __init__(self, imagem, disco=None):
        extras = ["-drive", f"file={disco},format=raw,if=ide"] if disco else []
        self.qemu = subprocess.Popen(
            ["qemu-system-x86_64", "-kernel", imagem, "-m", "128M", "-serial", "stdio",
             "-display", "none", "-no-reboot", "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"] + extras,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        # Texto cru (com as cores): o QEMU escreve a serial byte a byte, e
        # as sequências ANSI só podem ser tiradas do texto já acumulado.
        self.saida = ""
        self.lido = 0  # até onde a saída já foi consumida
        self.mudou = threading.Condition()
        threading.Thread(target=self._ler, daemon=True).start()

    def _ler(self):
        decodificador = codecs.getincrementaldecoder("utf-8")("replace")
        while True:
            dados = self.qemu.stdout.read1(4096)
            with self.mudou:
                if not dados:
                    self.saida += "\0"  # marca de fim
                else:
                    self.saida += decodificador.decode(dados).replace("\r", "")
                self.mudou.notify_all()
            if not dados:
                return

    def esperar(self, condicao, segundos, descricao):
        """Espera a saída ainda não consumida satisfazer `condicao`."""
        limite = time.monotonic() + segundos
        with self.mudou:
            while True:
                novo = ANSI.sub("", self.saida[self.lido:])
                if PANICO in novo:
                    time.sleep(0.2)
                    raise Falha("pânico do kernel:\n" + ANSI.sub("", self.saida[self.lido:]))
                if condicao(novo):
                    return novo
                if novo.endswith("\0"):
                    raise Falha(f"o QEMU terminou esperando {descricao}:\n{novo}")
                restante = limite - time.monotonic()
                if restante <= 0:
                    raise Falha(f"tempo esgotado esperando {descricao}; saída até agora:\n{novo}")
                self.mudou.wait(restante)

    def esperar_prompt(self, segundos=TEMPO_COMANDO):
        """Devolve a saída até o próximo prompt (sem ele) e a consome. Como o
        prompt é o fim da saída (o sh espera o próximo comando), tudo o que
        chegou até aqui é consumido."""
        novo = self.esperar(lambda t: PROMPT.search(t), segundos, "o prompt")
        with self.mudou:
            self.lido = len(self.saida)
        return novo[:PROMPT.search(novo).start()]

    def digitar(self, texto):
        self.qemu.stdin.write(texto.encode())
        self.qemu.stdin.flush()

    def desligar(self):
        """Sai do sh e desliga pelo monitor, para o disco ficar marcado
        como desmontado direito."""
        self.digitar("exit\r")
        self.esperar(lambda t: MONITOR.search(t), TEMPO_COMANDO, "o monitor do kernel")
        self.lido = len(self.saida)
        self.digitar("desligar\r")
        try:
            self.qemu.wait(TEMPO_COMANDO)
        except subprocess.TimeoutExpired:
            raise Falha("o sistema não desligou")

    def encerrar(self):
        self.qemu.kill()
        self.qemu.wait()


def ler_testes(caminho):
    """Lista de (linha, comando, teclas, linhas esperadas); o comando
    None marca um `@reiniciar`. Devolve também se o arquivo usa disco."""
    casos = []
    disco = True
    with open(caminho, encoding="utf-8") as arquivo:
        for numero, linha in enumerate(arquivo, 1):
            linha = linha.rstrip("\n")
            if linha == "@sem-disco":
                disco = False
            elif linha == "@reiniciar":
                casos.append((numero, None, [], []))
            elif linha.startswith("$ "):
                casos.append((numero, linha[2:], [], []))
            elif linha.startswith("#"):
                continue
            elif not casos:
                if linha.strip():
                    raise Falha(f"{caminho}:{numero}: texto antes do primeiro comando")
            elif linha in TECLAS and not casos[-1][3]:
                casos[-1][2].append(TECLAS[linha])
            else:
                casos[-1][3].append(linha)
    for _, _, _, esperado in casos:
        while esperado and not esperado[-1].strip():
            esperado.pop()  # linhas vazias no fim só separam os casos
    return casos, disco


def verificar_disco(disco):
    """O e2fsck (sem alterar nada) precisa achar o disco consistente."""
    resultado = subprocess.run(["e2fsck", "-fn", disco], capture_output=True, text=True,
                               env=dict(os.environ, LC_ALL="C"))
    if resultado.returncode != 0:
        raise Falha(f"e2fsck achou problemas no disco:\n{resultado.stdout}{resultado.stderr}")


def casa(real, esperado):
    """Compara uma linha; só o `*` é especial (casa com qualquer trecho)."""
    padrao = ".*".join(re.escape(parte) for parte in esperado.split("*"))
    return re.fullmatch(padrao, real) is not None


def comparar(esperado, real):
    real = [linha.rstrip() for linha in real]
    esperado = [linha.rstrip() for linha in esperado]
    if len(real) == len(esperado) and all(casa(r, e) for r, e in zip(real, esperado)):
        return None
    largura = max([len(e) for e in esperado] + [8])
    linhas = [f"  {'esperado':<{largura}} | real"]
    for i in range(max(len(real), len(esperado))):
        e = esperado[i] if i < len(esperado) else "∅"
        r = real[i] if i < len(real) else "∅"
        marca = " " if i < len(real) and i < len(esperado) and casa(r, e) else "✗"
        linhas.append(f"{marca} {e:<{largura}} | {r}")
    return "\n".join(linhas)


def rodar_arquivo(caminho, imagem, gravar):
    casos, usa_disco = ler_testes(caminho)
    pasta = tempfile.mkdtemp(prefix="ferrugem-teste-")
    disco = None
    if usa_disco:
        disco = os.path.join(pasta, "disco.img")
        shutil.copy(DISCO_TESTE, disco)
    maquina = Maquina(imagem, disco)
    falhas = 0
    try:
        maquina.esperar_prompt(TEMPO_BOOT)
        for numero, comando, teclas, esperado in casos:
            if comando is None:
                if gravar:
                    print("@reiniciar")
                maquina.desligar()
                if disco:
                    verificar_disco(disco)
                maquina = Maquina(imagem, disco)
                maquina.esperar_prompt(TEMPO_BOOT)
                continue
            maquina.digitar(comando + "\r")
            for tecla in teclas:
                # Dá tempo ao comando de começar antes da tecla.
                time.sleep(0.5)
                maquina.digitar(tecla)
            saida = maquina.esperar_prompt()
            # A primeira linha é o eco do próprio comando.
            real = saida.split("\n", 1)[1] if "\n" in saida else ""
            real = real.rstrip("\n").split("\n") if real.strip() else []
            if gravar:
                print(f"$ {comando}")
                for linha in real:
                    print(linha.rstrip())
                continue
            diferenca = comparar(esperado, real)
            if diferenca:
                falhas += 1
                print(f"\n✗ {caminho}:{numero}: $ {comando}\n{diferenca}")
        maquina.desligar()
        if disco:
            verificar_disco(disco)
    except Falha as erro:
        print(f"\n✗ {caminho}: {erro}")
        falhas += 1
    finally:
        maquina.encerrar()
        shutil.rmtree(pasta)
    return sum(1 for caso in casos if caso[1] is not None), falhas


def main():
    argumentos = sys.argv[1:]
    gravar = "--gravar" in argumentos
    arquivos = [a for a in argumentos if a != "--gravar"]
    if not arquivos:
        pasta = os.path.join(RAIZ, "tests")
        arquivos = sorted(os.path.join(pasta, nome) for nome in os.listdir(pasta) if nome.endswith(".txt"))

    imagem = KERNEL + ".bin"
    subprocess.run(["objcopy", "-O", "binary", KERNEL, imagem], check=True)
    subprocess.run([os.path.join(RAIZ, "scripts/disco.sh"), DISCO_TESTE], check=True, stdout=subprocess.DEVNULL,
                   env=dict(os.environ, FERRUGEM_DISCO_NOVO="1", FERRUGEM_DISCO_MB="32"))

    total = falhas = 0
    for caminho in arquivos:
        inicio = time.monotonic()
        casos, erros = rodar_arquivo(caminho, imagem, gravar)
        total += casos
        falhas += erros
        if not gravar:
            situacao = "ok" if erros == 0 else f"{erros} falha(s)"
            print(f"{os.path.relpath(caminho, RAIZ)}: {casos} comandos, {situacao} "
                  f"({time.monotonic() - inicio:.1f} s)")
    if not gravar:
        print(f"\n{total} comandos, {falhas} falha(s)")
    return 1 if falhas else 0


if __name__ == "__main__":
    sys.exit(main())
