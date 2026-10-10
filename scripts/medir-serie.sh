#!/bin/bash
# Mide una serie de commits de ESTE repo: para cada commit del rango compila su
# propio binario y lo corre sobre el mismo corpus, una salida por commit.
#
# Por que cambio (07-oct-2026): la version anterior hacia `cd ~/difuzz-main2` y
# copiaba el [patch.crates-io] de ~/difuzz-main para medir cada commit de difuzz
# contra SWC main (~/swc-main). Esas carpetas ya no estan en ~/ (viven en
# versiones-intermedias/, que no se toca) y SWC dejo de ser el objetivo: el
# minifier sale de las versiones fijas de Cargo.toml de cada commit. Lo que queda
# con sentido es comparar commits de difuzz entre si (la serie de mejoras).
# Como scripts/matriz.sh (M7): sin `cd` ni rutas fijas; todo por variable.
#
# Cada commit se extrae con `git archive` a <salida>/src-<commit> (el arbol del
# commit, sin el indice ni lo no commiteado) y se compila con
# `cargo build --release`. El target es compartido (TARGET): las dependencias
# con la misma version se reusan; difuzz se recompila siempre porque cada commit
# tiene su propia ruta.
#
# uso: [REPO=.] CORPUS=... [TARGET=<salida>/target] [ARGS="..."] medir-serie.sh RANGO [dir_salida]
#   RANGO       rango de git (p. ej. 9ac4db5..HEAD o HEAD~5..HEAD), en orden cronologico
#   ARGS        argumentos extra para cada binario (p. ej. "--jobs 6 --no-cache"). Los
#               commits viejos no conocen las opciones nuevas (--minifier, --carril...)
#   dir_salida  default: out-serie
#   CORPUS      obligatoria (si falta, se usa DIFUZZ_CORPUS; sin ninguna de las dos sale con 2). El
#               de los fixtures de SWC es <clon de swc>/crates/swc_ecma_minifier/tests
#   -h, --help  esta ayuda (sale con 0)
# Escribe en dir_salida: bin-<c>, build-<c>.log, run-<c>.log, stats-<c>.txt, out-<c>/.
set -u
case ${1:-} in -h|--help) rg '^#( |$)' -N "$0" | rg -v '^#!'; exit 0;; esac
[ -n "${1:-}" ] || { echo "falta RANGO: uso: medir-serie.sh RANGO [dir_salida] (ver --help)" >&2; exit 2; }
RANGO=$1
OUT=${2:-out-serie}
REPO=${REPO:-$(dirname "$0")/..}
CORPUS=${CORPUS:-${DIFUZZ_CORPUS:-}}
[ -n "$CORPUS" ] || { echo "falta el corpus: CORPUS=DIR o DIFUZZ_CORPUS=DIR (ver --help)" >&2; exit 2; }
ARGS=${ARGS:-}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
TARGET=${TARGET:-$OUT/target}

commits=$(git -C "$REPO" rev-list --reverse "$RANGO") || exit 1
[ -n "$commits" ] || { echo "rango vacio: $RANGO"; exit 1; }
for c in $commits; do
  c=$(git -C "$REPO" rev-parse --short "$c")
  echo "=== $c $(git -C "$REPO" log -1 --format=%s "$c")"
  src="$OUT/src-$c"
  rm -rf "$src" && mkdir -p "$src"
  # -m: mtime de ahora, no la del commit (si no, cargo puede dar por fresco un
  # src-<c> re-extraido y no recompilar).
  git -C "$REPO" archive "$c" | tar -x -m -C "$src" || { echo "  git archive fallo"; break; }
  if ! CARGO_TARGET_DIR="$TARGET" cargo build --release --manifest-path "$src/Cargo.toml" > "$OUT/build-$c.log" 2>&1; then
    echo "  NO COMPILA: ver $OUT/build-$c.log"; tail -20 "$OUT/build-$c.log"; break
  fi
  if [[ $(< "$OUT/build-$c.log") != *"Compiling difuzz"* ]]; then
    echo "  NO RECOMPILO difuzz: el binario seria el de otro commit"; break
  fi
  cp "$TARGET/release/difuzz" "$OUT/bin-$c"
  # ARGS sin comillas a proposito: se parte en palabras.
  # shellcheck disable=SC2086
  "$OUT/bin-$c" "$CORPUS" --out "$OUT/out-$c" $ARGS 2> "$OUT/run-$c.log" > "$OUT/stats-$c.txt"
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "  el binario salio con $rc (opcion que ese commit no conoce?): ver $OUT/run-$c.log"
    tail -3 "$OUT/run-$c.log"
    continue
  fi
  printf '%s\n' "$(< "$OUT/stats-$c.txt")"
done
