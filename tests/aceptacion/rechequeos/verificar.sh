#!/usr/bin/env bash
# Aceptacion de los re-chequeos (R1, C4): cada caso combina un bug real de oxc con algo que prende un
# flag de asuncion. "positivo": el flag explica la divergencia (known/); "negativo" y "miente": el
# flag no la explica (div/). Corre corpus/ en las 4 celdas de oxc y compara con esperado.tsv.
#
# uso: tests/aceptacion/rechequeos/verificar.sh [opciones] [BINARIO] [DIR_SALIDA]
# Mismas opciones y defaults que tests/golden/verificar.sh (este script solo le pasa el corpus, el
# esperado y las celdas; sin la comparacion de modos opt-in). Sale con 0 si todo coincide.
set -u
here=$(cd "$(dirname "$0")" && pwd)
# 10.10 (B-H6 de la revision B de 10.1, el mismo defecto que tests/codegen/verificar.sh): la ayuda es
# la de este script (antes, la del golden).
for a in "$@"; do
  case $a in -h|--help) rg '^#( |$)' -N "$0"; exit 0;; esac
done
exec "$here/../../golden/verificar.sh" \
  --corpus "$here/corpus" \
  --esperado "$here/esperado.tsv" \
  --celdas "oxc-a-core_defaults oxc-a-carril_a oxc-b-core_defaults oxc-b-carril_a" \
  --sin-modos \
  "$@"
