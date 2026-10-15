#!/usr/bin/env bash
# barrido-sintetico.sh: genera una tanda con scripts/generar.cjs y la corre con difuzz (oxc).
# Desde el 10-oct (MANUAL.md, Corpus) el corpus sintetico es la fuente de exploracion y la regresion de lo ya reportado.
#
# uso: barrido-sintetico.sh --semilla N [opciones] regresion OUT [BIN]
#      barrido-sintetico.sh --semilla N [opciones] explorar FAMILIA OUT [BIN]
#
#   regresion   1000 casos con --con-conocidos (todas las familias): los patrones de los bugs ya
#               reportados salen a proposito, asi que la tanda TIENE que dar div/. Se corre con cada
#               version nueva de oxc y antes de cada PR: un bug que deja de aparecer fue arreglado
#               (o el generador se rompio).
#   explorar F  5000 casos de la familia F (F1..F10), sin los patrones conocidos: busca bugs
#               nuevos. Criterio (10-oct): una familia con 0 bugs nuevos en 5000 casos filtrados se
#               retira de la exploracion y queda solo en la regresion.
#   OUT         directorio nuevo o vacio: OUT/corpus/ (la tanda), OUT/<celda>/ (salida de difuzz) y
#               OUT/TANDA.txt (registro: modo, familia, semilla, cantidad, sha256 del generador y
#               del arbol generado, comando, binario y su sha256, node, rc, segundos, stats)
#   BIN         el binario de difuzz (con BIN-oraculo al lado). Default: $DIFUZZ_BIN o
#               target/release/difuzz del repo de este script
#
# opciones:
#   --semilla N   OBLIGATORIA (entero): semilla base del generador. Misma semilla, misma tanda,
#                 byte a byte (TANDA.txt guarda el sha256 del arbol para comprobarlo)
#   --cantidad N  otro tamano de tanda (default: 1000 en regresion, 5000 en explorar)
#   --carril K    a (script, .cjs) o b (modulo ES, .mjs). Default b, como las tandas de t-gen; los
#                 programas valen en los dos carriles ("use strict" arriba)
#   --jobs N      workers de difuzz (default 8)
#   -h, --help    esta ayuda (sale con 0)
#
# Sin sandbox: los programas son generados aca, sin require ni E/S. Con --minifier oxc y el preset
# por defecto (carril_a).
# Cuanto tarda (MEDIDO el 10-oct, --jobs 8, maquina cargada): 61 s la regresion y 144 s una
# exploracion.
# Codigo de salida: el de difuzz (0 corrida entera, con o sin hallazgos; 130 interrumpida con
# Ctrl-C, con TANDA.txt escrito), 2 por un error de uso o
# si OUT ya tiene algo, 1 si el generador falla.
# Dependencias: bash >= 4, node, fd, rg, sort, xargs, sha256sum, realpath, date, tee.
set -uo pipefail

aqui=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
semilla=""
cantidad=""
carril=b
jobs=8
pos=()

uso() {
  if [[ ${1:-2} == 0 ]]; then cabecera; exit 0; fi
  cabecera >&2; exit 2
}
# La ayuda es el primer bloque de comentarios de este archivo (sin el shebang ni lo de abajo).
cabecera() {
  local l
  while IFS= read -r l; do
    [[ $l == '#!'* ]] && continue
    [[ $l == '#'* ]] || break
    l=${l#\#}; printf '%s\n' "${l# }"
  done < "$0"
}

while (($#)); do
  case $1 in
    --semilla) semilla=${2:-}; [[ $semilla =~ ^-?[0-9]+$ ]] || { echo "--semilla espera un entero" >&2; exit 2; }; shift 2;;
    --cantidad) cantidad=${2:-}; [[ $cantidad =~ ^[1-9][0-9]*$ ]] || { echo "--cantidad espera un entero >= 1" >&2; exit 2; }; shift 2;;
    --carril) carril=${2:-}; [[ $carril == a || $carril == b ]] || { echo "--carril espera a o b" >&2; exit 2; }; shift 2;;
    --jobs) jobs=${2:-}; [[ $jobs =~ ^[1-9][0-9]*$ ]] || { echo "--jobs espera un entero >= 1" >&2; exit 2; }; shift 2;;
    -h|--help) uso 0;;
    --) shift; pos+=("$@"); break;;
    -*) echo "opcion desconocida: $1 (ver --help)" >&2; exit 2;;
    *) pos+=("$1"); shift;;
  esac
done

[[ -n $semilla ]] || { echo "falta --semilla N: la semilla tiene que quedar registrada (ver --help)" >&2; exit 2; }
modo=${pos[0]:-}
familia=""
case $modo in
  regresion) ((${#pos[@]} == 2 || ${#pos[@]} == 3)) || uso
             out=${pos[1]}; bin=${pos[2]:-}; cantidad=${cantidad:-1000};;
  explorar) ((${#pos[@]} == 3 || ${#pos[@]} == 4)) || uso
            familia=${pos[1]}; out=${pos[2]}; bin=${pos[3]:-}; cantidad=${cantidad:-5000}
            [[ $familia =~ ^F([1-9]|10)$ ]] || { echo "familia desconocida: $familia (F1..F10)" >&2; exit 2; };;
  *) uso;;
esac
bin=${bin:-${DIFUZZ_BIN:-$aqui/../target/release/difuzz}}
[[ -x $bin ]] || { echo "no es ejecutable: $bin (BIN o DIFUZZ_BIN; ver --help)" >&2; exit 2; }
for dep in node fd rg sort xargs sha256sum realpath date tee; do
  command -v "$dep" >/dev/null || { echo "falta la dependencia: $dep" >&2; exit 2; }
done
bin=$(realpath "$bin")
generador=$aqui/generar.cjs
mkdir -p "$out" || exit 2
out=$(realpath "$out")
# OUT nuevo o vacio: una tanda no se mezcla con otra (ni con la salida de una corrida anterior).
if [[ -n $(fd -H -d 1 . "$out") ]]; then
  echo "OUT no esta vacio: $out (usar un directorio nuevo por tanda)" >&2; exit 2
fi

gen_args=("$out/corpus" "$cantidad" --semilla "$semilla")
if [[ $modo == regresion ]]; then gen_args+=(--con-conocidos); else gen_args+=(--familias "$familia"); fi
node "$generador" "${gen_args[@]}" || { echo "el generador fallo" >&2; exit 1; }
# sha256 del arbol generado: el de cada input.js en orden de ruta, y el de esa lista.
arbol=$(cd "$out/corpus" && fd -t f -0 . | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | rg -o '^[0-9a-f]+')

args=("$out/corpus" --out "$out" --minifier oxc --carril "$carril" --jobs "$jobs")
inicio=$(date -Is)
t0=$(date +%s)
# tee -i: con Ctrl-C, ver el comentario igual en barrido-real.sh (sin -i: rc 101 y .stdout vacio).
"$bin" "${args[@]}" | tee -i "$out/difuzz.stdout"
rc=${PIPESTATUS[0]}
t1=$(date +%s)

{
  echo "modo: $modo"
  [[ -n $familia ]] && echo "familia: $familia"
  echo "semilla: $semilla"
  echo "cantidad: $cantidad"
  printf 'generador:'; printf ' %q' node "$generador" "${gen_args[@]}"; echo
  echo "generador_sha256: $(sha256sum "$generador" | rg -o '^[0-9a-f]+')"
  echo "arbol_sha256: $arbol"
  echo "inicio: $inicio"
  echo "fin: $(date -Is)"
  echo "segundos: $((t1 - t0))"
  echo "rc: $rc"
  printf 'comando:'; printf ' %q' "$bin" "${args[@]}"; echo
  echo "binario_sha256: $(sha256sum "$bin" | rg -o '^[0-9a-f]+')"
  [[ -x $bin-oraculo ]] && echo "oraculo_sha256: $(sha256sum "$bin-oraculo" | rg -o '^[0-9a-f]+')"
  echo "node: $(node --version)"
  echo "stdout:"
  rg -N '' "$out/difuzz.stdout"
} > "$out/TANDA.txt"
echo "registro: $out/TANDA.txt"
exit "$rc"
