#!/usr/bin/env python3
"""Gera as imagens de raiz/usr/share/imagens: papéis de parede, um fractal
e imagens de teste do decodificador de PNG (cada pixel é uma fórmula que o
teste do kernel confere). Os PNGs gerados ficam no repositório; este script
só é preciso para refazê-los (precisa do Pillow: pip install pillow)."""
import math
import os
import random
import struct
import zlib
from PIL import Image, ImageDraw, ImageFilter

DESTINO = os.path.join(os.path.dirname(__file__), "..", "raiz", "usr", "share", "imagens")


def mistura(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def papel_ferrugem(l=1024, a=768):
    im = Image.new("RGB", (l, a))
    px = im.load()
    for y in range(a):
        for x in range(l):
            t = (x / l * 0.6 + y / a * 0.4)
            px[x, y] = mistura((22, 58, 72), (120, 45, 20), t ** 1.4)
    d = ImageDraw.Draw(im, "RGBA")
    for r, cor in [(520, (183, 65, 14, 40)), (360, (255, 140, 60, 30)), (220, (255, 200, 120, 25))]:
        d.ellipse((l - r * 0.9, a - r * 0.9, l + r * 1.1, a + r * 1.1), fill=cor)
    return im.filter(ImageFilter.GaussianBlur(2))


def papel_noite(l=1024, a=768):
    im = Image.new("RGB", (l, a))
    px = im.load()
    for y in range(a):
        for x in range(l):
            px[x, y] = mistura((8, 12, 30), (30, 40, 90), y / a)
    d = ImageDraw.Draw(im)
    aleatorio = random.Random(7)
    for _ in range(400):
        x, y = aleatorio.randrange(l), aleatorio.randrange(a * 3 // 4)
        b = aleatorio.randrange(120, 255)
        d.point((x, y), fill=(b, b, min(255, b + 20)))
    d.ellipse((760, 90, 860, 190), fill=(235, 230, 200))
    d.ellipse((790, 80, 890, 180), fill=mistura((8, 12, 30), (30, 40, 90), 0.15))
    return im


def papel_aurora(l=1024, a=768):
    im = Image.new("RGB", (l, a))
    px = im.load()
    for y in range(a):
        for x in range(l):
            onda = math.sin(x / 140 + math.sin(y / 90)) * 0.5 + 0.5
            px[x, y] = mistura(mistura((10, 30, 40), (20, 120, 100), onda * (1 - y / a)), (40, 20, 60), (y / a) ** 2)
    return im.filter(ImageFilter.GaussianBlur(3))


def mandelbrot(l=640, a=480):
    im = Image.new("RGB", (l, a))
    px = im.load()
    for y in range(a):
        for x in range(l):
            c = complex(-2.3 + x / l * 3.2, -1.2 + y / a * 2.4)
            z, n = 0, 0
            while abs(z) < 2 and n < 80:
                z = z * z + c
                n += 1
            if n == 80:
                px[x, y] = (10, 10, 20)
            else:
                t = n / 80
                px[x, y] = (int(255 * t ** 0.5), int(140 * t), int(60 + 120 * (1 - t)))
    return im


def paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    return a if pa <= pb and pa <= pc else (b if pb <= pc else c)


def salvar_entrelacado(im, caminho):
    """PNG RGBA com entrelaçamento Adam7 (o Pillow não grava assim), usando
    os cinco filtros de linha em rodízio, para testar todos no decodificador."""
    l, a = im.size
    passadas = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    cru = bytearray()
    contador = 0
    for x0, y0, dx, dy in passadas:
        w, h = len(range(x0, l, dx)), len(range(y0, a, dy))
        if w == 0 or h == 0:
            continue
        anterior = bytes(w * 4)
        for y in range(y0, a, dy):
            linha = bytes(v for x in range(x0, l, dx) for v in im.getpixel((x, y)))
            filtro = contador % 5
            contador += 1
            saida = bytearray()
            for i, v in enumerate(linha):
                esq = linha[i - 4] if i >= 4 else 0
                cima = anterior[i]
                diag = anterior[i - 4] if i >= 4 else 0
                pred = [0, esq, cima, (esq + cima) // 2, paeth(esq, cima, diag)][filtro]
                saida.append((v - pred) % 256)
            cru += bytes([filtro]) + saida
            anterior = linha

    def chunk(tipo, dados):
        return struct.pack(">I", len(dados)) + tipo + dados + struct.pack(">I", zlib.crc32(tipo + dados))

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", l, a, 8, 6, 0, 0, 1))
    png += chunk(b"IDAT", zlib.compress(bytes(cru), 9)) + chunk(b"IEND", b"")
    open(caminho, "wb").write(png)


def testes():
    # RGBA entrelaçado (Adam7): (x*7, y*11, (x+y)*3, 255-x) módulo 256
    im = Image.new("RGBA", (37, 23))
    for y in range(23):
        for x in range(37):
            im.putpixel((x, y), ((x * 7) % 256, (y * 11) % 256, ((x + y) * 3) % 256, (255 - x) % 256))
    salvar_entrelacado(im, os.path.join(DESTINO, "testes", "rgba-entrelacado.png"))
    # Paleta de 16 cores, a cor 0 transparente: índice = (x + 2y) % 16
    p = Image.new("P", (20, 10))
    p.putpalette(sum([[i * 16, 255 - i * 16, (i * 37) % 256] for i in range(16)], []))
    for y in range(10):
        for x in range(20):
            p.putpixel((x, y), (x + 2 * y) % 16)
    p.save(os.path.join(DESTINO, "testes", "paleta.png"), transparency=0, bits=4)
    # Cinza de 16 bits: valor = x*4000 + y*100
    g = Image.new("I;16", (16, 8))
    for y in range(8):
        for x in range(16):
            g.putpixel((x, y), x * 4000 + y * 100)
    g.save(os.path.join(DESTINO, "testes", "cinza16.png"))


if __name__ == "__main__":
    os.makedirs(os.path.join(DESTINO, "testes"), exist_ok=True)
    for nome, f in [("papel-ferrugem", papel_ferrugem), ("papel-noite", papel_noite), ("papel-aurora", papel_aurora),
                    ("mandelbrot", mandelbrot)]:
        f().save(os.path.join(DESTINO, nome + ".png"), optimize=True)
    testes()
