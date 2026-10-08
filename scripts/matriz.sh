#!/bin/bash
# Corre la matriz {carril_a, core_defaults} x {tal_cual, en_funcion} con el binario
# de ~/difuzz-main2 (contra ~/swc-main) sobre corpus-4b42, y cruza los div/.
# Las celdas comparten la cache de A ($OUT/cache-a) y el indice ($OUT/indice);
# el cruce lo imprime `difuzz --cruce`, sin volver a correr nada.
# PENDIENTE (otra tarea): ~/difuzz-main2 ahora vive en versiones-intermedias/ y
# su binario no tiene --cache/--indice/--cruce; las rutas quedan como estaban.
# uso: matriz.sh [dir_salida]   (default: out-matriz)
set -u
cd ~/difuzz-main2 || exit 1
BIN=./target/release/difuzz
OUT=${1:-out-matriz}
CELLS=(core_defaults+en_funcion core_defaults+tal_cual carril_a+en_funcion carril_a+tal_cual)
mkdir -p "$OUT"
rm -rf "${OUT:?}/indice"
for cell in "${CELLS[@]}"; do
  echo "=== $cell"
  rm -rf "${OUT:?}/$cell"
  "$BIN" corpus-4b42 --config "${cell%+*}" --forma "${cell#*+}" --out "$OUT/$cell" \
    --cache "$OUT/cache-a" --indice "$OUT/indice" \
    2> "$OUT/$cell.log" > "$OUT/$cell.stats"
  cat "$OUT/$cell.stats"
done

echo
echo "=== C=core_defaults, A=carril_a; fn=en_funcion, tc=tal_cual"
echo "    C.fn o C.tc con X = uso comun por construccion (defaults de @swc/core)"
"$BIN" --cruce "$OUT/indice"
