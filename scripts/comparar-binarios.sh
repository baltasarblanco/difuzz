#!/usr/bin/env bash
# comparar-binarios.sh: arnes de no regresion entre dos binarios de difuzz.
#
# Corre el corpus completo con BIN_BASE y con BIN_NUEVO en cada celda pedida y
# compara, caso por caso, todo lo que define la clasificacion. Pensado para un
# refactor que NO debe cambiar ningun resultado.
#
# uso: comparar-binarios.sh [opciones] BIN_BASE BIN_NUEVO DIR_SALIDA [celda...]
#
#   celda          una cadena con los argumentos de la celda, entre comillas:
#                  "MINIFIER CARRIL [flags...]", p. ej. "oxc b --invocar".
#                  Default: "oxc a" "oxc b" "swc a" "oxc b --invocar" "swc b"
#
# opciones:
#   --jobs N                 workers de difuzz (default 8). Las corridas son secuenciales.
#   --corpus DIR             default: $DIFUZZ_CORPUS. Sin ninguno de los dos sale con 2. El corpus
#                            de los fixtures de SWC es <clon de swc>/crates/swc_ecma_minifier/tests
#   -h, --help               esta ayuda (sale con 0)
#   --reusar                 no volver a correr un lado/celda si su salida ya esta completa (rc 0)
#   --base-de DIR            tomar las corridas del lado base de DIR (otro DIR_SALIDA/base) en vez de correrlas
#   --solo-comparar          no correr nada: comparar lo que ya hay en DIR_SALIDA
#   --ignorar-campo NOMBRE   en meta.txt (y CORRIDA.txt): quitar la linea "NOMBRE: ..."; si es
#                            cabecera de seccion ("NOMBRE:" sin valor) quita tambien las lineas
#                            siguientes hasta la primera linea en blanco. Repetible.
#                            P. ej. --ignorar-campo clasificacion
#   --ignorar-linea REGEX    quitar las lineas que matcheen REGEX (rg) en meta.txt, stdout,
#                            stderr y CORRIDA.txt antes de comparar. Repetible.
#   --permitir-sin-corrida   aceptar un lado sin CORRIDA.txt (difuzz anterior a 55a2d43, salida
#                            plana sin <celda>/): se exige rc 0 y una linea total= en stdout.
#   --max-detalle N          lineas de diff por archivo en el informe (default 40)
#
# salida: DIR_SALIDA/{base,nuevo}/<etiqueta>/ (el --out de cada corrida), <etiqueta>.{stdout,stderr,rc}
# y DIR_SALIDA/DIFERENCIAS.txt (lo mismo que se imprime).
# codigo de salida: 0 sin diferencias, 1 con diferencias, 2 si no se pudo comparar
# (corrida incompleta, estructura irreconocible, argumentos).
#
# Que compara, por celda:
#   - rc de los dos procesos;
#   - stdout entero (las lineas de stats: total=, analisis..., oraculo..., invocar..., mutaciones...);
#   - CORRIDA.txt (estado, casos, procesados, backend, node, stats), sin inicio/fin/pid/comando;
#   - stderr como multiconjunto de lineas (orden libre: los workers intercalan), sin el progreso [n/m];
#   - el conjunto de casos de div, known, panic, asserts, lint, recursos y codegen (10.1, --crudo;
#     perdidos, nuevos, movidos);
#   - por caso en el mismo destino: meta.txt normalizado entero, el conjunto de archivos del caso
#     y el contenido de cada uno (sha256 tras normalizar rutas);
#   - descartes.tsv como (caso, causa) ordenado; known-sintomas.tsv ordenado; lint-triage.tsv si hay.
# Que ignora (normalizacion):
#   - rutas: la raiz de la celda de cada lado -> <CELDA> (asi la salida plana y la por celda se
#     comparan), el --out -> <OUT>, el directorio del binario -> <BIN>;
#   - tiempos: "<n>(ns|us|µs|ms|s)" -> <T> en meta.txt y en stderr; "<n> MiB" -> <N> MiB
#     (mensajes de recursos/ y del aviso de memoria);
#   - meta.txt: el hash de la cobertura de bloques de V8 ("<hash16> <n> contra <hash16> <n>" ->
#     "<HASH> <n> contra <HASH> <n>"): no es determinista entre corridas del mismo binario (10.10);
#   - CORRIDA.txt: inicio, fin, pid, comando;
#   - stdout: cache_a_hits=N (con --no-cache siempre 0, pero no es determinista con cache);
#   - lo que se pida con --ignorar-campo / --ignorar-linea.
#
#   - stderr: la ruta del oraculo ("oraculo sin debug assertions: <ruta>" -> <ORACULO>), que
#     depende de donde vive el binario y no de su comportamiento;
#   - el archivo temporal de cada worker en los stacks de a.exc/b.exc
#     (/tmp/difuzz-<pid>-tmp/difuzz-<pid>-w<n>.cjs): pid y worker cambian entre corridas;
#   - stderr: "borre N temporales de corridas anteriores cortadas" (main.rs:583). Depende de lo
#     que otros procesos dejaron en /tmp, no del binario: MEDIDO no determinista (base contra
#     si mismo: 2 vs 8, 3 vs 0, 2 vs 1 en cuatro celdas).
#
# Dependencias: bash >= 4, eza, rg, sd (>= 1.0, para -F), sort, comm, cut, tr, diff, sha256sum,
# date, mktemp, realpath.
# Para listar directorios se usa `eza -1 --only-dirs` y nunca `fd -x`: fd -x corre en paralelo e
# intercala lineas (ya produjo un falso "2 perdidos" en este proyecto).
set -uo pipefail

corpus=${DIFUZZ_CORPUS:-}
jobs=8
reusar=0
solo_comparar=0
base_de=""
sin_corrida=0
max_detalle=40
campos_ign=()
lineas_ign=()
pos=()

# uso [CODIGO]: 0 (--help) a stdout; si no, a stderr y sale con 2.
uso() {
  if [[ ${1:-2} == 0 ]]; then rg '^#( |$)' -N "$0" | sd '(?m)^# ?' ''; exit 0; fi
  rg '^#( |$)' -N "$0" | sd '(?m)^# ?' '' >&2; exit 2
}

while (($#)); do
  case $1 in
    --jobs) jobs=${2:?--jobs necesita valor}; shift 2;;
    --corpus) corpus=${2:?}; shift 2;;
    --reusar) reusar=1; shift;;
    --base-de) base_de=${2:?}; shift 2;;
    --solo-comparar) solo_comparar=1; shift;;
    --ignorar-campo) campos_ign+=("${2:?}"); shift 2;;
    --ignorar-linea) lineas_ign+=("${2:?}"); shift 2;;
    --permitir-sin-corrida) sin_corrida=1; shift;;
    --max-detalle) max_detalle=${2:?}; shift 2;;
    -h|--help) uso 0;;
    --) shift; pos+=("$@"); break;;
    --*) echo "opcion desconocida: $1" >&2; uso;;
    *) pos+=("$1"); shift;;
  esac
done
((${#pos[@]} >= 3)) || uso
[[ -n $corpus ]] || { echo "falta el corpus: --corpus DIR o DIFUZZ_CORPUS=DIR (ver --help)" >&2; exit 2; }
bin_base=$(realpath "${pos[0]}") || exit 2
bin_nuevo=$(realpath "${pos[1]}") || exit 2
salida=${pos[2]}
celdas=("${pos[@]:3}")
((${#celdas[@]})) || celdas=("oxc a" "oxc b" "swc a" "oxc b --invocar" "swc b")
for b in "$bin_base" "$bin_nuevo"; do [[ -x $b ]] || { echo "no es ejecutable: $b" >&2; exit 2; }; done
for dep in eza rg sd sort comm diff sha256sum; do
  command -v "$dep" >/dev/null || { echo "falta la dependencia: $dep" >&2; exit 2; }
done
mkdir -p "$salida" || exit 2
salida=$(realpath "$salida")
informe=$salida/DIFERENCIAS.txt
: > "$informe"
tmp=$(mktemp -d) || exit 2
trap 'rm -rf "$tmp"' EXIT

difs=0        # diferencias encontradas
errores=0     # celdas que no se pudieron comparar

say() { printf '%s\n' "$*" | tee -a "$informe"; }
dif() { difs=$((difs + 1)); say "  DIF $*"; }
# Muestra hasta max_detalle lineas de un diff, indentadas.
detalle() { local n=0 l; while IFS= read -r l; do n=$((n + 1)); ((n > max_detalle)) && { say "      (... recortado)"; break; }; say "      $l"; done; }

etiqueta() { local e=${1// /-}; e=${e//---/-}; e=${e//--/-}; printf '%s' "${e//[^A-Za-z0-9_.-]/_}"; }

# --- correr -----------------------------------------------------------------
correr() { # lado bin celda
  local lado=$1 bin=$2 celda=$3 et out rc t0
  et=$(etiqueta "$celda"); out=$salida/$lado/$et
  if ((reusar)) && [[ -f $out.rc && $(<"$out.rc") == 0 ]]; then
    say "  ($lado/$et: reusada)"; return
  fi
  rm -rf "$out" "$out".{stdout,stderr,rc}
  mkdir -p "$salida/$lado"
  t0=$(date +%s)
  local w; read -r -a w <<< "$celda"   # MINIFIER CARRIL [flags...]
  ((${#w[@]} >= 2)) || { say "  ERROR: celda mal formada: \"$celda\" (MINIFIER CARRIL [flags...])"; exit 2; }
  "$bin" "$corpus" --out "$out" --minifier "${w[0]}" --carril "${w[1]}" "${w[@]:2}" --no-cache --jobs "$jobs" \
    > "$out.stdout" 2> "$out.stderr"
  rc=$?
  echo "$rc" > "$out.rc"
  say "  corrida $lado/$et: rc=$rc $(( $(date +%s) - t0 ))s"
}

# --- normalizacion ----------------------------------------------------------
# Quita los campos y secciones ignorados. Lee stdin.
quitar_campos() {
  # Las lineas en blanco se retienen hasta ver la siguiente: si es una seccion
  # ignorada, el blanco que la separaba se va con ella.
  local skip=0 blancos=0 line nombre rest c ign
  while IFS= read -r line || [[ -n $line ]]; do
    if ((skip)); then [[ -z $line ]] && skip=0; continue; fi
    if [[ -z $line ]]; then blancos=$((blancos + 1)); continue; fi
    if [[ $line =~ ^([A-Za-z_][A-Za-z0-9_]*):(.*)$ ]]; then
      nombre=${BASH_REMATCH[1]}; rest=${BASH_REMATCH[2]}; ign=0
      for c in "${campos_ign[@]}"; do [[ $c == "$nombre" ]] && ign=1; done
      if ((ign)); then [[ -z ${rest// /} ]] && { skip=1; blancos=0; }; continue; fi
    fi
    while ((blancos)); do printf '\n'; blancos=$((blancos - 1)); done
    printf '%s\n' "$line"
  done
  while ((blancos)); do printf '\n'; blancos=$((blancos - 1)); done
}
quitar_lineas() {
  if ((${#lineas_ign[@]})); then
    local args=() r; for r in "${lineas_ign[@]}"; do args+=(-e "$r"); done
    rg -v "${args[@]}" || true
  else
    rg '' || true
  fi
}
rutas() { # raiz_celda out bindir
  sd -F -- "$1" '<CELDA>' | sd -F -- "$2" '<OUT>' | sd -F -- "$3" '<BIN>' \
    | sd '(?m)^(oraculo sin debug assertions:) .*$' '$1 <ORACULO>' \
    | sd 'difuzz-[0-9]+-tmp/difuzz-[0-9]+-w[0-9]+' 'difuzz-<PID>-tmp/difuzz-<PID>-w<W>'
}
tiempos() {
  sd '\b[0-9]+(\.[0-9]+)?(ns|us|µs|ms|s)\b' '<T>' | sd '\b[0-9]+ MiB\b' '<N> MiB'
}
# 10.10: el hash de la cobertura de bloques de V8 (`<hash16> <rangos> contra <hash16> <rangos>`,
# recheck::comparar_caminos y camino_sensor_i3) no es determinista entre corridas del mismo binario
# (MEDIDO en la revision de 10.1: 27 diferencias base contra base en la aceptacion, 5 celdas). Se
# cambia solo el hash; la cantidad de rangos se compara.
cobertura() {
  sd '\b[0-9a-f]{16} ([0-9]+) contra [0-9a-f]{16} ([0-9]+)\b' '<HASH> $1 contra <HASH> $2'
}
# normalizar ARCHIVO OUT BINDIR TIPO   (TIPO: meta | stdout | stderr | corrida | crudo)
normalizar() {
  local f=$1 out=$2 bd=$3 tipo=$4 celda=$2
  # La raiz de la celda (<out>/<celda>/ o --out mismo en la salida plana) -> <CELDA>.
  [[ $out == "$salida/base/"* || $out == "$dir_base/"* ]] && celda=$rb_actual || celda=$rn_actual
  [[ -f $f ]] || { echo "<no existe>"; return; }
  case $tipo in
    meta)    quitar_campos < "$f" | quitar_lineas | rutas "$celda" "$out" "$bd" | tiempos | cobertura;;
    stdout)  quitar_lineas < "$f" | rutas "$celda" "$out" "$bd" | sd ' cache_a_hits=[0-9]+' '' | tiempos;;
    stderr)  rg -v -e '^\[[0-9]+/[0-9]+\]$' -e '^borre [0-9]+ temporales de corridas anteriores cortadas' "$f" | quitar_lineas | rutas "$celda" "$out" "$bd" | tiempos | sort;;
    corrida) rg -v '^(inicio|fin|pid|comando):' "$f" | quitar_campos | quitar_lineas | rutas "$celda" "$out" "$bd" | sd ' cache_a_hits=[0-9]+' '';;
    crudo)   rutas "$celda" "$out" "$bd" < "$f";;
  esac
}

# Raiz de la celda dentro del --out de una corrida: <out>/<celda>/ (desde 55a2d43) o el
# --out mismo (salida plana anterior). Imprime "ruta<TAB>estructura".
raiz_celda() {
  local out=$1 dirs
  if [[ -e $out/descartes.tsv || -d $out/div ]]; then printf '%s\tplana\n' "$out"; return 0; fi
  [[ -d $out ]] || return 1
  mapfile -t dirs < <(eza -1 --only-dirs "$out" | rg -v '^(cache-a)?$')
  ((${#dirs[@]} == 1)) || { echo "  $out: ${#dirs[@]} directorios de celda (${dirs[*]}), se esperaba 1" >&2; return 1; }
  printf '%s\tpor_celda\n' "$out/${dirs[0]}"
}

casos_de() { [[ -d $1 ]] && eza -1 --only-dirs "$1" | rg -v '^$' | sort; }

comparar_textos() { # titulo archivo_base archivo_nuevo
  if ! diff -q "$2" "$3" >/dev/null; then
    dif "$1"
    diff -U0 --label base --label nuevo "$2" "$3" | rg -v '^(@@|---|\+\+\+)' | detalle
  fi
}

DESTINOS=(div known panic asserts lint recursos codegen)

comparar_celda() { # celda
  local celda=$1 et ob on rb rn eb en ba na d c f
  et=$(etiqueta "$celda"); ob=$dir_base/$et; on=$salida/nuevo/$et
  say ""; say "== celda \"$celda\" ($et)"
  local rcb rcn; rcb="?"; rcn="?"; [[ -f $ob.rc ]] && rcb=$(<"$ob.rc"); [[ -f $on.rc ]] && rcn=$(<"$on.rc")
  [[ $rcb == "$rcn" ]] || dif "rc del proceso: base=$rcb nuevo=$rcn"
  IFS=$'\t' read -r rb eb < <(raiz_celda "$ob") || { say "  ERROR: no encuentro la celda de base en $ob"; errores=$((errores + 1)); return; }
  IFS=$'\t' read -r rn en < <(raiz_celda "$on") || { say "  ERROR: no encuentro la celda nueva en $on"; errores=$((errores + 1)); return; }
  say "  base:  $rb ($eb)"; say "  nuevo: $rn ($en)"
  rb_actual=$rb; rn_actual=$rn
  [[ $eb == "$en" ]] || say "  AVISO: estructuras de --out distintas (base $eb, nuevo $en); se compara la raiz de cada celda"
  # Corrida completa en los dos lados.
  for lado in base nuevo; do
    local r o e; if [[ $lado == base ]]; then r=$rb; o=$ob; e=$rcb; else r=$rn; o=$on; e=$rcn; fi
    if [[ -f $r/CORRIDA.txt ]]; then
      rg -q '^estado: completa$' "$r/CORRIDA.txt" || { say "  ERROR: $lado: CORRIDA.txt no dice 'estado: completa'"; errores=$((errores + 1)); return; }
    elif ((sin_corrida)) && [[ $e == 0 ]] && rg -q '^total=' "$o.stdout"; then
      say "  AVISO: $lado sin CORRIDA.txt (--permitir-sin-corrida): rc 0 y total= en stdout"
    else
      say "  ERROR: $lado sin CORRIDA.txt (salida de un difuzz anterior? usar --permitir-sin-corrida) o incompleta"
      errores=$((errores + 1)); return
    fi
  done
  local bdb bdn; bdb=$(dirname "$bin_base_usado"); bdn=$(dirname "$bin_nuevo")
  # Stats y logs.
  normalizar "$ob.stdout" "$ob" "$bdb" stdout > "$tmp/b"; normalizar "$on.stdout" "$on" "$bdn" stdout > "$tmp/n"
  comparar_textos "stdout (lineas de stats)" "$tmp/b" "$tmp/n"
  if [[ -f $rb/CORRIDA.txt && -f $rn/CORRIDA.txt ]]; then
    normalizar "$rb/CORRIDA.txt" "$ob" "$bdb" corrida > "$tmp/b"; normalizar "$rn/CORRIDA.txt" "$on" "$bdn" corrida > "$tmp/n"
    comparar_textos "CORRIDA.txt" "$tmp/b" "$tmp/n"
  fi
  normalizar "$ob.stderr" "$ob" "$bdb" stderr > "$tmp/b"; normalizar "$on.stderr" "$on" "$bdn" stderr > "$tmp/n"
  comparar_textos "stderr (multiconjunto de lineas)" "$tmp/b" "$tmp/n"
  # Casos por destino: indice caso -> destino de cada lado.
  : > "$tmp/ib"; : > "$tmp/in"
  for d in "${DESTINOS[@]}"; do
    casos_de "$rb/$d" | while IFS= read -r c; do printf '%s\t%s\n' "$c" "$d"; done >> "$tmp/ib"
    casos_de "$rn/$d" | while IFS= read -r c; do printf '%s\t%s\n' "$c" "$d"; done >> "$tmp/in"
  done
  sort -o "$tmp/ib" "$tmp/ib"; sort -o "$tmp/in" "$tmp/in"
  local resumen=""
  for d in "${DESTINOS[@]}"; do
    resumen+=" $d=$(rg -c $'\t'"$d"'$' "$tmp/ib" || echo 0)/$(rg -c $'\t'"$d"'$' "$tmp/in" || echo 0)"
  done
  say "  destinos (base/nuevo):$resumen"
  # Perdidos, nuevos y movidos (un caso puede estar en mas de un destino: se compara el conjunto).
  local -A destb=() destn=()
  while IFS=$'\t' read -r c d; do destb[$c]+="${destb[$c]:+ }$d"; done < "$tmp/ib"
  while IFS=$'\t' read -r c d; do destn[$c]+="${destn[$c]:+ }$d"; done < "$tmp/in"
  local db dn
  while IFS= read -r c; do
    [[ -n $c ]] || continue
    db=${destb[$c]:-}; dn=${destn[$c]:-}
    if [[ -z $db ]]; then dif "caso NUEVO en [$dn]: $c"
    elif [[ -z $dn ]]; then dif "caso PERDIDO de [$db]: $c"
    elif [[ $db != "$dn" ]]; then dif "caso MOVIDO [$db] -> [$dn]: $c"
    fi
    # Contenido: para cada destino en que esta en los dos lados.
    for d in $db; do
      [[ " $dn " == *" $d "* ]] || { [[ -n $dn ]] && mostrar_meta "$c" "$rb/$d/$c" "$rn"; continue; }
      comparar_caso "$d/$c" "$rb/$d/$c" "$rn/$d/$c" "$ob" "$on" "$bdb" "$bdn"
    done
  done < <(cut -f1 "$tmp/ib" "$tmp/in" | sort -u)
  # descartes.tsv (caso, causa), sin elapsed_ms.
  for lado in b n; do
    local r; [[ $lado == b ]] && r=$rb || r=$rn
    if [[ -f $r/descartes.tsv ]]; then rg -v '^#' "$r/descartes.tsv" | cut -f1,2 | sort > "$tmp/d$lado"; else echo "<no existe>" > "$tmp/d$lado"; fi
  done
  say "  descartes (base/nuevo): $(rg -c . "$tmp/db" || echo 0)/$(rg -c . "$tmp/dn" || echo 0)"
  comparar_textos "descartes.tsv (caso, causa)" "$tmp/db" "$tmp/dn"
  for f in known-sintomas.tsv; do
    [[ -f $rb/$f || -f $rn/$f ]] || continue
    { [[ -f $rb/$f ]] && sort "$rb/$f" || echo "<no existe>"; } > "$tmp/b"
    { [[ -f $rn/$f ]] && sort "$rn/$f" || echo "<no existe>"; } > "$tmp/n"
    comparar_textos "$f (ordenado)" "$tmp/b" "$tmp/n"
  done
  if [[ -f $ob/lint-triage.tsv || -f $on/lint-triage.tsv ]]; then
    { [[ -f $ob/lint-triage.tsv ]] && sort "$ob/lint-triage.tsv" || echo "<no existe>"; } > "$tmp/b"
    { [[ -f $on/lint-triage.tsv ]] && sort "$on/lint-triage.tsv" || echo "<no existe>"; } > "$tmp/n"
    comparar_textos "lint-triage.tsv (ordenado)" "$tmp/b" "$tmp/n"
  fi
}

# Un caso movido: las lineas de clasificacion de cada lado, para leer el porque.
mostrar_meta() { # caso dir_base raiz_nueva
  local c=$1 d
  say "      base  $(rg -N '^(kind|known|nota|causa|flags_a|flags_b):' "$2/meta.txt" 2>/dev/null | tr '\n' ' ')"
  for d in "${DESTINOS[@]}"; do
    [[ -f $3/$d/$c/meta.txt ]] && say "      nuevo $(rg -N '^(kind|known|nota|causa|flags_a|flags_b):' "$3/$d/$c/meta.txt" | tr '\n' ' ') [$d]"
  done
}

comparar_caso() { # titulo dir_base dir_nuevo out_base out_nuevo bindir_base bindir_nuevo
  local t=$1 db=$2 dn=$3 f
  normalizar "$db/meta.txt" "$4" "$6" meta > "$tmp/mb"; normalizar "$dn/meta.txt" "$5" "$7" meta > "$tmp/mn"
  comparar_textos "meta.txt de $t" "$tmp/mb" "$tmp/mn"
  eza -1 -a --only-files "$db" | rg -v '^(meta\.txt)?$' | sort > "$tmp/fb"
  eza -1 -a --only-files "$dn" | rg -v '^(meta\.txt)?$' | sort > "$tmp/fn"
  if ! diff -q "$tmp/fb" "$tmp/fn" >/dev/null; then
    dif "archivos de $t: base [$(tr '\n' ' ' < "$tmp/fb")] nuevo [$(tr '\n' ' ' < "$tmp/fn")]"
  fi
  while IFS= read -r f; do
    [[ -n $f ]] || continue
    local hb hn
    hb=$(normalizar "$db/$f" "$4" "$6" crudo | sha256sum); hn=$(normalizar "$dn/$f" "$5" "$7" crudo | sha256sum)
    if [[ $hb != "$hn" ]]; then
      dif "contenido de $t/$f"
      diff -U0 --label base --label nuevo <(normalizar "$db/$f" "$4" "$6" crudo) <(normalizar "$dn/$f" "$5" "$7" crudo) | rg -v '^(@@|---|\+\+\+)' | detalle
    fi
  done < <(comm -12 "$tmp/fb" "$tmp/fn")
}

# --- principal --------------------------------------------------------------
say "comparar-binarios $(date -u +%FT%TZ)"
say "base:  $bin_base"; say "nuevo: $bin_nuevo"; say "salida: $salida"; say "corpus: $corpus  jobs: $jobs"
say "celdas: $(printf '"%s" ' "${celdas[@]}")"
((${#campos_ign[@]})) && say "campos ignorados: ${campos_ign[*]}"
((${#lineas_ign[@]})) && say "lineas ignoradas: ${lineas_ign[*]}"
[[ -d $corpus ]] || { say "ERROR: no existe el corpus $corpus"; exit 2; }

dir_base=$salida/base
bin_base_usado=$bin_base
if [[ -n $base_de ]]; then
  dir_base=$(realpath "$base_de") || exit 2
  say "lado base tomado de $dir_base (no se corre)"
fi
if ((!solo_comparar)); then
  for celda in "${celdas[@]}"; do
    [[ -n $base_de ]] || correr base "$bin_base" "$celda"
    correr nuevo "$bin_nuevo" "$celda"
  done
fi
for celda in "${celdas[@]}"; do comparar_celda "$celda"; done

say ""
if ((errores)); then say "RESULTADO: no se pudo comparar ($errores celdas con error), $difs diferencias en el resto"; exit 2; fi
if ((difs)); then say "RESULTADO: $difs diferencias"; exit 1; fi
say "RESULTADO: sin diferencias"
exit 0
