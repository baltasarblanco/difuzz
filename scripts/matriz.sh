#!/bin/bash
# Corre la matriz {carril_a, core_defaults} x {tal_cual, en_funcion} con un backend
# (MIN=swc|oxc) y un carril (CARRIL=a|b) sobre el corpus, y cruza los div/.
# Las celdas comparten la cache de A ($OUT/cache-a) y el indice ($OUT/indice);
# el cruce lo imprime `difuzz --cruce`, sin volver a correr nada.
# M7: sin `cd` ni rutas fijas; no borra el directorio de la celda (el reporter ya
# limpia div/, known/, panic/ y lint/), asi sobreviven known-sintomas.tsv y
# lint-triage.tsv entre corridas.
# uso: [BIN=...] [CORPUS=...] [MIN=swc|oxc] [CARRIL=a|b] matriz.sh [dir_salida]   (default: out-matriz)
set -u
BIN=${BIN:-$(dirname "$0")/../target/release/difuzz}
CORPUS=${CORPUS:-$HOME/swc-main/crates/swc_ecma_minifier/tests}
MIN=${MIN:-swc}
CARRIL=${CARRIL:-a}
OUT=${1:-out-matriz}
CELLS=(core_defaults+en_funcion core_defaults+tal_cual carril_a+en_funcion carril_a+tal_cual)
mkdir -p "$OUT"
for cell in "${CELLS[@]}"; do
  echo "=== $MIN $cell carril $CARRIL"
  "$BIN" "$CORPUS" --minifier "$MIN" --carril "$CARRIL" --config "${cell%+*}" --forma "${cell#*+}" \
    --out "$OUT/$cell" --cache "$OUT/cache-a" --indice "$OUT/indice" \
    2> "$OUT/$cell.log" > "$OUT/$cell.stats"
  printf '%s\n' "$(< "$OUT/$cell.stats")"
done

echo
echo "=== <backend> C=core_defaults, A=carril_a; fn=en_funcion, tc=tal_cual; B=carril_b"
echo "    letra por celda = kind (ver la leyenda de --cruce); C.fn o C.tc = uso comun por construccion"
"$BIN" --cruce "$OUT/indice"
