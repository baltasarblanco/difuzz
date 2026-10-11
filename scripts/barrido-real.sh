#!/usr/bin/env bash
# barrido-real.sh: corre difuzz sobre un corpus de codigo real con la configuracion que dio H1 y
# H2 (oxc, carril_a, --invocar --stubs) y SIEMPRE con --sandbox-red (D2, 10-oct).
#
# uso: barrido-real.sh [opciones] CORPUS OUT [BIN]
#      DIFUZZ_CORPUS=CORPUS barrido-real.sh [opciones] OUT
#
#   CORPUS  directorio con NNNN/input.js, armado con scripts/armar-corpus-real.py (que deja
#           fuentes.tsv con el origen de cada caso). Default: $DIFUZZ_CORPUS. Sin ninguno sale con 2
#   OUT     el --out de difuzz: la celda queda en OUT/<celda>/ y el registro de la corrida en
#           OUT/BARRIDO-carril_<K>.txt (fecha, comando, binario y su sha256, node, corpus, rc, segundos)
#   BIN     el binario de difuzz (con BIN-oraculo al lado, ver difuzz --help). Default: $DIFUZZ_BIN o
#           target/release/difuzz del repo de este script
#
# opciones:
#   --carril K  a (script, .cjs) para un corpus CJS o b (modulo ES, .mjs) para uno ESM. Default: el
#               de la columna forma de CORPUS/fuentes.tsv (cjs -> a, esm -> b; las dos -> sale con 2);
#               sin fuentes.tsv, a. Un --carril que contradice fuentes.tsv sale con 2
#   --jobs N    workers de difuzz (default 8)
#   --limit N   procesar solo los primeros N casos (pruebas)
#   -h, --help  esta ayuda (sale con 0)
#
# Fijo, sin opcion para cambiarlo: --minifier oxc --config carril_a --forma tal_cual --invocar
# --stubs --sandbox-red. El corpus es codigo ajeno: el sandbox con namespaces (sin red, sin
# sockets unix del sistema; node lee solo el preload, el programa y el directorio de trabajo del
# caso, y escribe solo en ese directorio; ver README "Sandbox") no se puede apagar
# desde aca (decision del 10-oct: --sandbox sigue apagado por defecto en difuzz, por
# comparabilidad de celdas y cache, y es obligatorio en este barrido). Si el sandbox no se puede
# armar (sin user namespaces), difuzz sale con 1 y este script tambien.
#
# Codigo de salida: el de difuzz (0 corrida entera, con o sin hallazgos; 1 error de entorno, p. ej.
# sandbox imposible; 130 interrumpida), o 2 por un error de uso de este script. Tambien 1 si difuzz
# termino con 0 pero su stdout no trae la linea "sandbox: modo=permisos+red" (no deberia pasar).
#
# Cuanto tarda (MEDIDO el 10-oct, --jobs 8, ver README "Corpus"): ~12 casos/s en CJS y ~25 en ESM;
# el pool local entero (18749 casos en 4 corpus) en 23 min 35 s.
# Dependencias: bash >= 4, rg, sort, tr, tee, sha256sum, realpath, date.
set -uo pipefail

aqui=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
corpus=${DIFUZZ_CORPUS:-}
carril=""
jobs=8
limit=""
pos=()

# uso [CODIGO]: 0 (--help) a stdout; si no, a stderr y sale con 2.
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
    --carril) carril=${2:-}; [[ $carril == a || $carril == b ]] || { echo "--carril espera a o b" >&2; exit 2; }; shift 2;;
    --jobs) jobs=${2:-}; [[ $jobs =~ ^[1-9][0-9]*$ ]] || { echo "--jobs espera un entero >= 1" >&2; exit 2; }; shift 2;;
    --limit) limit=${2:-}; [[ $limit =~ ^[1-9][0-9]*$ ]] || { echo "--limit espera un entero >= 1" >&2; exit 2; }; shift 2;;
    -h|--help) uso 0;;
    --) shift; pos+=("$@"); break;;
    -*) echo "opcion desconocida: $1 (este script no pasa opciones a difuzz; ver --help)" >&2; exit 2;;
    *) pos+=("$1"); shift;;
  esac
done

case ${#pos[@]} in
  1) [[ -n $corpus ]] || { echo "falta el corpus: CORPUS OUT o DIFUZZ_CORPUS=CORPUS (ver --help)" >&2; exit 2; }
     out=${pos[0]}; bin=${DIFUZZ_BIN:-$aqui/../target/release/difuzz};;
  2|3) corpus=${pos[0]}; out=${pos[1]}; bin=${pos[2]:-${DIFUZZ_BIN:-$aqui/../target/release/difuzz}};;
  *) uso;;
esac
[[ -d $corpus ]] || { echo "el corpus no es un directorio: $corpus" >&2; exit 2; }
[[ -x $bin ]] || { echo "no es ejecutable: $bin (BIN o DIFUZZ_BIN; ver --help)" >&2; exit 2; }
for dep in rg sort tr tee sha256sum realpath date; do
  command -v "$dep" >/dev/null || { echo "falta la dependencia: $dep" >&2; exit 2; }
done
corpus=$(realpath "$corpus")
bin=$(realpath "$bin")

# El carril sale de fuentes.tsv (columna forma de las filas que entraron al corpus).
fuentes=$corpus/fuentes.tsv
if [[ -f $fuentes ]]; then
  # Columnas: caso sha256 bytes forma n_requires origen [estado]; caso "-" = no entro al corpus.
  formas=$(rg -N -v '^caso\t' "$fuentes" | rg -N -v '^-\t' | rg -N -o -r '$1' '^[^\t]*\t[^\t]*\t[^\t]*\t([^\t]*)\t' | sort -u | tr '\n' ' ')
  case $formas in
    "cjs ") deducido=a;;
    "esm ") deducido=b;;
    "") deducido="";;
    *) echo "fuentes.tsv mezcla formas ($formas): armar un corpus por forma" >&2; exit 2;;
  esac
  if [[ -n $deducido && -n $carril && $carril != "$deducido" ]]; then
    echo "--carril $carril contradice fuentes.tsv (forma ${formas% }): CJS va en a, ESM en b" >&2; exit 2
  fi
  carril=${carril:-$deducido}
fi
carril=${carril:-a}

mkdir -p "$out" || exit 2
out=$(realpath "$out")
registro=$out/BARRIDO-carril_$carril.txt
args=("$corpus" --out "$out" --minifier oxc --config carril_a --forma tal_cual --carril "$carril"
      --invocar --stubs --sandbox-red --jobs "$jobs")
[[ -n $limit ]] && args+=(--limit "$limit")

inicio=$(date -Is)
t0=$(date +%s)
"$bin" "${args[@]}" | tee "$out/barrido-carril_$carril.stdout"
rc=${PIPESTATUS[0]}
t1=$(date +%s)

{
  echo "inicio: $inicio"
  echo "fin: $(date -Is)"
  echo "segundos: $((t1 - t0))"
  echo "rc: $rc"
  printf 'comando:'; printf ' %q' "$bin" "${args[@]}"; echo
  echo "binario_sha256: $(sha256sum "$bin" | rg -o '^[0-9a-f]+')"
  [[ -x $bin-oraculo ]] && echo "oraculo_sha256: $(sha256sum "$bin-oraculo" | rg -o '^[0-9a-f]+')"
  echo "node: $(node --version 2>/dev/null || echo '?')"
  echo "corpus: $corpus"
  [[ -f $fuentes ]] && echo "fuentes_sha256: $(sha256sum "$fuentes" | rg -o '^[0-9a-f]+')"
  echo "stdout:"
  rg -N '' "$out/barrido-carril_$carril.stdout"
} > "$registro"
echo "registro: $registro"

if ((rc == 0)) && ! rg -q '^sandbox: modo=permisos\+red ' "$out/barrido-carril_$carril.stdout"; then
  echo "difuzz termino sin la linea 'sandbox: modo=permisos+red': no se puede afirmar que corrio con el sandbox" >&2
  exit 1
fi
exit "$rc"
