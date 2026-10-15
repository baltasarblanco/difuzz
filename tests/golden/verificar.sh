#!/usr/bin/env bash
# Prueba de regresion de deteccion de difuzz sobre bugs conocidos (golden).
# Corre un corpus en varias celdas (por defecto el golden en 8: swc|oxc x carril a|b x
# core_defaults|carril_a), clasifica cada caso en cada celda y lo compara con esperado.tsv.
#
# uso: tests/golden/verificar.sh [opciones] [BINARIO] [DIR_SALIDA]
#   BINARIO     default: ${CARGO_TARGET_DIR:-<repo>/target}/release/difuzz
#   DIR_SALIDA  default: un directorio nuevo bajo ${TMPDIR:-/tmp} (se borra al final si todo pasa)
# opciones:
#   --corpus DIR      default: corpus/ junto a este script
#   --esperado TSV    default: esperado.tsv junto a este script
#   --celdas "C..."   celdas <minifier>-<carril>-<preset> separadas por espacios (default: las 8)
#   --sin-modos       no correr la comparacion de modos opt-in (--invocar, --stubs)
#   --difuzz "ARGS"   argumentos extra para difuzz en cada celda y en los modos (p. ej. "--crudo", 10.1);
#                     repetible: se acumulan
#   --jobs N          workers de difuzz (default ${VERIFICAR_JOBS:-4})
# Sale con 0 si todas las comparaciones coinciden (y con --help/-h), 1 si hay alguna diferencia,
# 2 por argumentos.
# El oraculo sin debug assertions (cargo build --profile oraculo) es parte de lo esperado: difuzz
# lo busca junto al binario o en DIFUZZ_ORACULO (ver tests/README.md).
# Herramientas: bash, fd, rg (no usa cat/grep/find/sed/awk).
set -u
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
corpus=$here/corpus
esperado=$here/esperado.tsv
jobs=${VERIFICAR_JOBS:-4}
modos=1
extra=()
celdas=()
for m in swc oxc; do for k in a b; do for p in core_defaults carril_a; do celdas+=("$m-$k-$p"); done; done; done
pos=()
# 10.10 (B-H6 de la revision B de 10.1): una opcion sin valor es un error de uso (2), no el rc 1 de
# `${2:?}`. 10.2b (B-H6 de la revision B del bloque 10): tampoco otra opcion como valor (salvo en
# --difuzz, cuyo valor son opciones de difuzz), y --jobs tiene que ser un entero positivo.
valor() { # opcion valor
  [ -n "${2:-}" ] || { echo "$1 necesita valor" >&2; exit 2; }
  [[ $1 == --difuzz || $2 != -* ]] || { echo "$1 necesita valor, no otra opcion: $2" >&2; exit 2; }
}
# 10.2b (B-H5): la ayuda es la cabecera de este archivo (los comentarios hasta la primera linea
# que no lo es), sin los comentarios internos de mas abajo.
ayuda() {
  local l
  while IFS= read -r l; do
    case $l in '#!'*) ;; '#'*) printf '%s\n' "$l";; *) break;; esac
  done < "$0"
}
while (($#)); do
  case $1 in
    --corpus) valor "$@"; corpus=$2; shift 2;;
    --esperado) valor "$@"; esperado=$2; shift 2;;
    --celdas) valor "$@"; read -r -a celdas <<< "$2"; shift 2;;
    --sin-modos) modos=0; shift;;
    # 10.2b (B-H4): se acumulan (tests/codegen/verificar.sh pasa --crudo y el usuario puede sumar otros).
    --difuzz) valor "$@"; read -r -a mas <<< "$2"; extra+=("${mas[@]}"); shift 2;;
    --jobs) valor "$@"; [[ $2 =~ ^[1-9][0-9]*$ ]] || { echo "--jobs necesita un entero positivo: $2" >&2; exit 2; }; jobs=$2; shift 2;;
    -h|--help) ayuda; exit 0;;
    --*) echo "opcion desconocida: $1" >&2; exit 2;;
    *) pos+=("$1"); shift;;
  esac
done
bin=${pos[0]:-${CARGO_TARGET_DIR:-$repo/target}/release/difuzz}
out=${pos[1]:-}
borrar_out=0
if [ -z "$out" ]; then out=$(mktemp -d "${TMPDIR:-/tmp}/difuzz-golden.XXXXXX"); borrar_out=1; fi
[ -x "$bin" ] || { echo "no encuentro el binario: $bin (cargo build --release, o pasalo como argumento)" >&2; exit 2; }
[ -d "$corpus" ] || { echo "no existe el corpus: $corpus" >&2; exit 2; }
[ -f "$esperado" ] || { echo "no existe el esperado: $esperado" >&2; exit 2; }
((${#celdas[@]})) || { echo "--celdas vacio" >&2; exit 2; }
mkdir -p "$out" || exit 2

declare -A exp
while IFS=$'\t' read -r caso celda clase; do
  [[ -z "$caso" || "$caso" == \#* ]] && continue
  exp["$caso|$celda"]=$clase
done < "$esperado"

# -u (= -H -I): como src/corpus.rs, tambien los directorios ocultos (p. ej. jquery/.19 de la muestra) y
# sin mirar .gitignore. Sin -u, un caso oculto se corria pero no se comparaba.
mapfile -t casos < <(fd -u -t f -g input.js "$corpus" -x dirname {} | while read -r d; do echo "${d#"$corpus"/}"; done | sort)

# b2-a (R1-H09): difuzz escribe en <--out>/<celda>/. Cada --out de este script tiene una sola celda
# (y --no-cache: sin cache-a), asi que el directorio de la celda es su unico subdirectorio.
celdir() { # $1 = --out de la celda
  local x; mapfile -t x < <(fd -t d -d 1 . "$1" | sort)
  if [ ${#x[@]} -eq 1 ]; then echo "${x[0]%/}"; else echo "$1/__no_hay_un_solo_subdirectorio__"; fi
}

# 10.1 (--crudo): codegen/ es ortogonal (como lint/, el caso cuenta tambien donde termino): va
# delante de la clase del caso, codegen:<kind>+<clase>. Sin --crudo no hay codegen/ y no cambia nada.
clase_de() { # $1 = --out de la celda, $2 = caso
  local o g=
  o=$(celdir "$1")
  if [ -d "$o/codegen/$2" ]; then
    g="codegen:$(rg -o -r '$1' '^kind: (.*)$' "$o/codegen/$2/meta.txt" | rg -v '^$')+"
  fi
  echo "$g$(clase_caso "$1" "$2")"
}

clase_caso() { # $1 = --out de la celda, $2 = caso
  local o c=$2 k
  o=$(celdir "$1")
  if [ -d "$o/div/$c" ]; then
    k=$(rg -o -r '$1' '^kind: (.*)$' "$o/div/$c/meta.txt" | rg -v '^$'); echo "div:$k"; return
  fi
  [ -d "$o/panic/$c" ] && { echo panic; return; }
  # b2-int (b2-b, R1-H08/R2-H04): timeout o memoria del minificador van a recursos/, no a panic/.
  [ -d "$o/recursos/$c" ] && { echo recursos; return; }
  # b5 (R1-H05): panic solo con debug assertions que con el oraculo no diverge va a asserts/.
  # Antes este script no lo miraba y lo leia como "igual".
  [ -d "$o/asserts/$c" ] && { echo asserts; return; }
  [ -d "$o/known/$c" ] && { echo known; return; }
  [ -d "$o/lint/$c" ] && { echo lint; return; }
  # M7 (revision de los bloques 4-6): comparacion exacta del nombre del caso (antes iba sin escapar en una regex).
  k=
  if [ -f "$o/descartes.tsv" ]; then
    while IFS=$'\t' read -r ca cau _; do
      [ "$ca" == "$c/input.js" ] && [ -n "$cau" ] && { k=$cau; break; }
    done < "$o/descartes.tsv"
  fi
  [ -n "$k" ] && { echo "descarte:$k"; return; }
  echo igual
}

fallas=0; total=0
# Una fila de esperado.tsv que no corresponde a ningun caso o celda corrida es un error del esperado.
for key in "${!exp[@]}"; do
  c=${key%%|*}; cel=${key#*|}
  hay_c=0; for x in "${casos[@]}"; do [ "$x" == "$c" ] && hay_c=1; done
  hay_cel=0; for x in "${celdas[@]}"; do [ "$x" == "$cel" ] && hay_cel=1; done
  if [ $hay_c -eq 0 ] || [ $hay_cel -eq 0 ]; then echo "esperado.tsv: fila sin caso o celda corrida: $c $cel"; fallas=$((fallas+1)); fi
done
printf '%-28s' caso
for c in "${celdas[@]}"; do printf ' %-22s' "$c"; done; echo
sin_oraculo=0
for cel in "${celdas[@]}"; do
  IFS=- read -r m k p <<< "$cel"
  "$bin" "$corpus" --out "$out/$cel" --minifier "$m" --carril "$k" --config "$p" --no-cache --jobs "$jobs" \
    "${extra[@]}" > "$out/$cel.log" 2>&1
  rc=$?
  if [ $rc -ne 0 ]; then echo "celda $cel: difuzz salio con $rc (ver $out/$cel.log)"; fallas=$((fallas+1)); fi
  # b2-a (R2-H05): la marca de corrida tiene que decir completa.
  e=$(rg -o -r '$1' '^estado: (.*)$' "$(celdir "$out/$cel")/CORRIDA.txt" 2>/dev/null)
  if [ "$e" != "completa" ]; then echo "celda $cel: CORRIDA.txt dice '${e:-sin marca}'"; fallas=$((fallas+1)); fi
  rg -q 'no hay oraculo sin debug assertions' "$out/$cel.log" && sin_oraculo=1
done
if [ $sin_oraculo -eq 1 ]; then
  echo "AVISO: difuzz corrio sin oraculo sin debug assertions (ver los .log): lo esperado lo asume."
  echo "       Compilalo con 'cargo build --profile oraculo' o pasalo con DIFUZZ_ORACULO=RUTA."
fi
for caso in "${casos[@]}"; do
  printf '%-28s' "$caso"
  for cel in "${celdas[@]}"; do
    total=$((total+1))
    got=$(clase_de "$out/$cel" "$caso")
    want=${exp["$caso|$cel"]:-igual}
    if [ "$got" == "$want" ]; then printf ' %-22s' "ok $got"; else printf ' %-22s' "MAL $got!=$want"; fallas=$((fallas+1)); fi
  done; echo
done
# Las stats de cada celda tienen que cerrar. MEDIDO en el corpus completo (07-oct): LINT es un
# contador ortogonal (chequeo estatico; un caso puede estar en div/ y en lint/ a la vez, p. ej.
# swc/B fixture/issues/12188). Desde R2-H06 (b6-usuario) descartes.tsv SI lista mudo_sin_bindings,
# ilegible y panic_del_fuzzer (antes se sumaban aparte mudos_sin_bindings + ilegibles), asi que
#   total = iguales + DIVERGENCIAS + conocidas + PANICS + RECURSOS + filas(descartes.tsv)
# (la misma identidad que documenta el USAGE de difuzz, sin --mutaciones).
# b2-int: RECURSOS (b2-b) es un destino terminal como PANICS (recursos/), no un contador ortogonal.
# Los descartes con causa `analisis` (b2-c) ya son filas de descartes.tsv. Los contadores de c y d
# (casos_con_fallo, analisis_swc, bloques) van en lineas aparte y no entran. Los de asserts/ van en
# la linea "oraculo (R1-H05):" y no entran (se cuentan dentro de iguales).
for cel in "${celdas[@]}"; do
  s=$(rg '^total=' "$out/$cel.log")
  v() { rg -o -r '$1' "(?:^| )$1=([0-9]+)" <<< "$s" | rg -v '^$' | { read -r x; echo "${x:-0}"; }; }
  t=$(v total); ig=$(v iguales); dv=$(v DIVERGENCIAS); kn=$(v conocidas)
  pa=$(v PANICS); re=$(v RECURSOS); ndesc=$(rg -c -v '^#' "$(celdir "$out/$cel")/descartes.tsv" 2>/dev/null || echo 0)
  suma=$((ig+dv+kn+pa+re+ndesc))
  if [ "$t" != "$suma" ]; then echo "celda $cel: stats no cierran: total=$t != $ig+$dv+$kn+$pa+$re+$ndesc=$suma"; fallas=$((fallas+1)); fi
done
# Los modos opt-in no pueden perder lo que la celda base encuentra (H01: --invocar lo perdia).
# Corre oxc-b-core_defaults y oxc-a-core_defaults con cada modo y exige que cada div/panic de la base siga en div/panic.
# 10.10 (B-H4 de la revision B de 10.1): con los mismos --difuzz que la celda base, y se compara el destino de A contra
# B (clase_caso), sin el codegen/ ortogonal: antes, con --difuzz "--crudo" la clase de la base empezaba con codegen: y
# el caso se salteaba sin aviso.
if [ $modos -eq 1 ]; then
for k in a b; do for modo in "--invocar" "--stubs"; do
  cel="oxc-$k-core_defaults"; tag="$cel${modo// /}"
  [ -d "$out/$cel" ] || { echo "modo $tag: falta la celda base $cel (usar --sin-modos)"; fallas=$((fallas+1)); continue; }
  "$bin" "$corpus" --out "$out/$tag" --minifier oxc --carril "$k" --config core_defaults --no-cache --jobs "$jobs" $modo \
    "${extra[@]}" > "$out/$tag.log" 2>&1
  rc=$?
  if [ $rc -ne 0 ]; then echo "modo $tag: difuzz salio con $rc (ver $out/$tag.log)"; fallas=$((fallas+1)); continue; fi
  for caso in "${casos[@]}"; do
    base=$(clase_caso "$out/$cel" "$caso"); got=$(clase_caso "$out/$tag" "$caso")
    case "$base" in div:*|panic) total=$((total+1))
      if [ "$base" != "$got" ]; then echo "MAL $tag $caso: base=$base modo=$got"; fallas=$((fallas+1)); fi ;;
    esac
  done
done; done
fi
echo "comparaciones=$total fallas=$fallas salida=$out"
if [ $fallas -eq 0 ] && [ $borrar_out -eq 1 ]; then rm -rf "$out"; fi
[ $fallas -eq 0 ]
