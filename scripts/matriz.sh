#!/bin/bash
# Corre la matriz {carril_a, core_defaults} x {tal_cual, en_funcion} con el binario
# de ~/difuzz-main2 (contra ~/swc-main) sobre corpus-4b42, y cruza los div/.
# uso: matriz.sh [dir_salida]   (default: out-matriz)
set -u
cd ~/difuzz-main2 || exit 1
BIN=./target/release/difuzz
OUT=${1:-out-matriz}
CELLS=(core_defaults+en_funcion core_defaults+tal_cual carril_a+en_funcion carril_a+tal_cual)
mkdir -p "$OUT"
for cell in "${CELLS[@]}"; do
  echo "=== $cell"
  rm -rf "${OUT:?}/$cell"
  "$BIN" corpus-4b42 --config "${cell%+*}" --forma "${cell#*+}" --out "$OUT/$cell" \
    2> "$OUT/$cell.log" > "$OUT/$cell.stats"
  cat "$OUT/$cell.stats"
done

echo
echo "=== divergencias por caso: X = diverge en la celda. Columnas: C=core_defaults, A=carril_a; fn=en_funcion, tc=tal_cual"
echo "    C.fn o C.tc con X = uso comun por construccion (defaults de @swc/core)"
printf '%-5s%-5s%-5s%-5s %-10s %s\n' C.fn C.tc A.fn A.tc kind caso
for c in $(for cell in "${CELLS[@]}"; do ls "$OUT/$cell/div" 2>/dev/null; done | sort -u); do
  row=""; kind=""
  for cell in "${CELLS[@]}"; do
    if [ -d "$OUT/$cell/div/$c" ]; then
      row+="X    "
      [ -z "$kind" ] && kind=$(sed -n 's/^kind: //p' "$OUT/$cell/div/$c/meta.txt")
    else
      row+=".    "
    fi
  done
  printf '%s %-10s %s\n' "$row" "$kind" "$c"
done | sort -r
