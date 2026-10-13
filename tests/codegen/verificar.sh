#!/usr/bin/env bash
# Corpus propio de codegen (10.1): 142 formas cuya semantica depende de la sintaxis exacta
# (parentesis, comas, directivas, ASI, claves computadas, escapes). Corre corpus/ con --crudo (el
# texto crudo contra el original que imprime el backend) en las 8 celdas (oxc|swc x carril a|b x
# core_defaults|carril_a) y compara cada caso con esperado.tsv. codegen/ es ortogonal: la clase es
# codegen:<kind>+<clase del caso> (ver tests/golden/verificar.sh). Origen, adaptaciones y lo que se
# espera de cada forma: README.md.
#
# uso: tests/codegen/verificar.sh [opciones] [BINARIO] [DIR_SALIDA]
# Mismas opciones y defaults que tests/golden/verificar.sh (este script le pasa el corpus, el
# esperado, las celdas y --difuzz "--crudo", que se suma a los --difuzz que se le pasen; sin la
# comparacion de modos opt-in). Sale con 0 si todo coincide.
set -u
here=$(cd "$(dirname "$0")" && pwd)
# 10.10 (B-H6 de la revision B de 10.1): la ayuda es la de este script (antes, la del golden).
# 10.2b (B-H5, B-H6 de la revision B del bloque 10): solo la cabecera, y solo `-h`/`--help` como
# opcion (no como valor de otra: `--corpus -h` es un error de uso, que da el golden con 2).
prev=
for a in "$@"; do
  case $prev in --corpus|--esperado|--celdas|--difuzz|--jobs) prev=; continue;; esac
  case $a in
    -h|--help)
      while IFS= read -r l; do case $l in '#!'*) ;; '#'*) printf '%s\n' "$l";; *) break;; esac; done < "$0"
      exit 0;;
  esac
  prev=$a
done
exec "$here/../golden/verificar.sh" \
  --corpus "$here/corpus" \
  --esperado "$here/esperado.tsv" \
  --celdas "oxc-a-core_defaults oxc-a-carril_a oxc-b-core_defaults oxc-b-carril_a swc-a-core_defaults swc-a-carril_a swc-b-core_defaults swc-b-carril_a" \
  --sin-modos \
  --difuzz "--crudo" \
  "$@"
