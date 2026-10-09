#!/usr/bin/env bash
# subir-oxc.sh: cambia la version de los crates de oxc y dice que cambio en los hallazgos (C7).
#
# uso: [VARIABLES] scripts/subir-oxc.sh VERSION DIR_SALIDA
#
#   VERSION     version nueva de los crates oxc_* (p. ej. 0.154.0). Sirve tambien para bajar.
#   DIR_SALIDA  todo lo que produce el procedimiento; se puede volver a correr el mismo comando:
#               retoma en el primer paso sin marca en DIR_SALIDA/estado/.
#
# Trabaja en un worktree temporal (DIR_SALIDA/wt, `git worktree add --detach` sobre REV): NO
# commitea, no toca el arbol desde el que se lo llama (ni su Cargo.toml ni su Cargo.lock). Lo no
# commiteado de ese arbol no entra (como scripts/medir-serie.sh).
#
# Pasos (cada uno deja DIR_SALIDA/estado/<paso>.ok):
#   1 worktree  git worktree add --detach DIR_SALIDA/wt REV
#   2 base      compila REV tal cual (release + perfil oraculo) y copia los dos binarios a
#               DIR_SALIDA/bin-base/{difuzz,difuzz-oraculo}; con BIN_BASE, copia ese (y BIN_BASE-oraculo)
#   3 subir     Cargo.toml: TODAS las lineas `oxc_* = "=VIEJA"` a "=VERSION" (tienen que ser iguales);
#               `cargo update -p` de esos crates; en src/ y tests/, "oxc_minifier VIEJA" ->
#               "oxc_minifier VERSION" (Minifier::version() en src/backend/mod.rs, el test
#               info_del_oraculo de src/subproceso.rs y un comentario de src/oracle.rs). Deja
#               DIR_SALIDA/lock.diff (paquetes del Cargo.lock que cambiaron). Tambien
#               probe/package.json: "oxc-minify": "VIEJA" -> "VERSION" (C1 de tests/muestra exige la
#               version npm == la del crate); su package-lock.json lo sube el paso 10 (RED=1)
#   4 compilar  cargo build --release && cargo build --profile oraculo (el oraculo TAMBIEN: valida al
#               binario principal por la version de los minifiers); copia a DIR_SALIDA/bin-nuevo/.
#               Si no compila (cambio de API de oxc) FRENA con codigo 3: arreglar el fuente en
#               DIR_SALIDA/wt (leyendo el fuente del crate nuevo en ~/.cargo/registry/src/) y volver a
#               correr el mismo comando. Los arreglos quedan en el parche del paso 11.
#   5 identidad `difuzz --preparar --info` de los cuatro binarios: version de oxc y debug_assertions
#   6 tests     cargo test y cargo clippy --all-targets -- -D warnings en el worktree (no frena)
#   7 golden    GOLDEN/verificar.sh y ACEPTACION/verificar.sh con el binario base y con el nuevo;
#               celdas que cambian (no frena, NO toca ningun esperado.tsv: si oxc arreglo un bug del
#               golden o de la aceptacion, lo avisa)
#   8 arnes     ARNES (comparar-binarios.sh) base contra nuevo sobre el corpus completo, ignorando la
#               linea "backend: oxc_minifier ..." (si no, todos los meta.txt difieren)
#   9 informe   DIR_SALIDA/diferencias.tsv e INFORME.txt: por celda, hallazgos (div/, panic/) que
#               DESAPARECEN, que APARECEN y casos que CAMBIAN de destino o kind; para cada uno corre
#               `--emitir` con los dos binarios y dice si el minificado cambio (si no cambio, la
#               diferencia no viene de oxc) y una pista de triage
#  10 red       solo con RED=1: npm install oxc-minify@VERSION en DIR_SALIDA/npm y corre
#               los repro de ENVIAR.md (REPRO/*.mjs) comparando con sus .out; sube oxc-minify en
#               probe/ del worktree (npm install --save-exact: package.json y package-lock.json) y
#               corre C1 de la muestra (cargo test c1_igual_a_oxc_minify_de_npm -- --ignored); con
#               gh, compara crates/oxc_minifier/docs/ASSUMPTIONS.md entre los tags crates_vVIEJA y
#               crates_vVERSION
#  11 parche    git diff del worktree -> DIR_SALIDA/subida-oxc-VERSION.patch; borra el worktree con
#               `git worktree remove --force` (el parche ya lo guarda) salvo MANTENER=1
#
# Variables:
#   REPO       repo de difuzz (default: el de este script)        REV     commit base (default HEAD)
#   TARGET     CARGO_TARGET_DIR (default: $CARGO_TARGET_DIR o DIR_SALIDA/target). Conviene uno que ya
#              tenga las dependencias compiladas: difuzz se recompila igual (otra ruta de fuente)
#   BIN_BASE   binario base ya compilado de REV (con su BIN_BASE-oraculo al lado); saltea la
#              compilacion del paso 2. Tiene que ser del MISMO codigo de difuzz que REV, si no el
#              informe mezcla cambios de difuzz con cambios de oxc (no se puede verificar solo)
#   CORPUS     default /home/baltasarblanco/swc-main/crates/swc_ecma_minifier/tests
#   JOBS       workers de difuzz en el arnes (default 8)
#   CELDAS     celdas del arnes separadas por ';' (default "oxc a;oxc b;oxc b --invocar;swc a;swc b";
#              las de swc son control: cambiar oxc no deberia moverlas)
#   ARNES_ARGS opciones extra para el arnes (p. ej. "--reusar" o "--base-de DIR/base")
#   ARNES      default REPO/scripts/comparar-binarios.sh
#   GOLDEN     default REPO/tests/golden (GOLDEN=no para saltearlo)
#   ACEPTACION default REPO/tests/aceptacion/rechequeos (ACEPTACION=no para saltearla)
#   ENVIAR     default <arbol principal>/.claude/trabajo-07oct/oxc/ENVIAR.md
#   REPRO      default <arbol principal>/.claude/trabajo-07oct/oxc/probe-npm/repro
#   RED=1      correr el paso 10 (npm y gh; NPM=1 es sinonimo)          MANTENER=1  no borrar el worktree
#
# Temporales: exporta TMPDIR=DIR_SALIDA/tmp (difuzz, cargo test y mktemp no dejan nada en /tmp).
# Codigo de salida: 0 todo verde y sin diferencias en hallazgos; 1 terminado con algo para revisar
# (tests, clippy, golden o diferencias); 2 argumentos o entorno; 3 no compila; 4 fallo un paso.
# Dependencias: bash >= 4, git, cargo, node, rg, sd (>= 1.0), eza, sort, comm, cut, diff, sha256sum,
# realpath, mktemp, join; npm y gh solo con RED=1. Los del arnes y el golden (ver sus cabeceras).
set -uo pipefail

uso() { rg '^#( |$)' -N "$0" | sd '(?m)^# ?' '' >&2; exit 2; }
(($# == 2)) || uso
[[ $1 == -h || $1 == --help ]] && uso
VERSION=$1
[[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "VERSION invalida: $VERSION" >&2; exit 2; }
for dep in git cargo node rg sd eza sort comm cut diff sha256sum realpath mktemp; do
  command -v "$dep" >/dev/null || { echo "falta la dependencia: $dep" >&2; exit 2; }
done

REPO=$(realpath "${REPO:-$(dirname "$0")/..}") || exit 2
REPO=$(git -C "$REPO" rev-parse --show-toplevel) || exit 2
# Los documentos de trabajo (.claude/, fuera de git: ENVIAR.md y sus repro) viven en el arbol
# principal, no en otro worktree. Golden, aceptacion y arnes estan en git (REPO).
PRINCIPAL=$(dirname "$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)")
REV=${REV:-HEAD}
REV_ID=$(git -C "$REPO" rev-parse --short "$REV^{commit}") || { echo "REV invalido: $REV" >&2; exit 2; }
mkdir -p "$2" || exit 2
OUT=$(realpath "$2")
TARGET=${TARGET:-${CARGO_TARGET_DIR:-$OUT/target}}
CORPUS=${CORPUS:-/home/baltasarblanco/swc-main/crates/swc_ecma_minifier/tests}
JOBS=${JOBS:-8}
CELDAS=${CELDAS:-oxc a;oxc b;oxc b --invocar;swc a;swc b}
ARNES_ARGS=${ARNES_ARGS:-}
ARNES=${ARNES:-$REPO/scripts/comparar-binarios.sh}
GOLDEN=${GOLDEN:-$REPO/tests/golden}
ACEPTACION=${ACEPTACION:-$REPO/tests/aceptacion/rechequeos}
ENVIAR=${ENVIAR:-$PRINCIPAL/.claude/trabajo-07oct/oxc/ENVIAR.md}
REPRO=${REPRO:-$PRINCIPAL/.claude/trabajo-07oct/oxc/probe-npm/repro}
RED=${RED:-${NPM:-0}}
MANTENER=${MANTENER:-0}
WT=$OUT/wt
EST=$OUT/estado
LOGS=$OUT/logs
mkdir -p "$EST" "$LOGS" "$OUT/tmp" || exit 2
export TMPDIR=$OUT/tmp
export CARGO_TARGET_DIR=$TARGET
LOG=$OUT/subir-oxc.log

say() { printf '%s\n' "$*" | tee -a "$LOG"; }
hecho() { [[ -f $EST/$1.ok ]]; }
marcar() { date -u +%FT%TZ > "$EST/$1.ok"; }
fallar() { say "ERROR: $*"; exit 4; }
# Lo que queda para revisar al final (no frena el procedimiento).
revisar=()
pendiente() { # sin duplicados al retomar
  say "  REVISAR: $*"
  rg -q -x -F -- "$*" "$EST/revisar.txt" 2>/dev/null || printf '%s\n' "$*" >> "$EST/revisar.txt"
}

# Una salida es de UNA version: no mezclar dos subidas en el mismo DIR_SALIDA.
if [[ -f $EST/version ]]; then
  [[ $(<"$EST/version") == "$VERSION $REV_ID" ]] || { echo "$OUT ya es de \"$(<"$EST/version")\"; usar otro DIR_SALIDA" >&2; exit 2; }
  say ""; say "== retomo $OUT ($(<"$EST/version"))"
else
  echo "$VERSION $REV_ID" > "$EST/version"
  : > "$EST/revisar.txt"
fi
say "subir-oxc $(date -u +%FT%TZ): VERSION=$VERSION REV=$REV_ID REPO=$REPO"
say "  salida=$OUT target=$TARGET corpus=$CORPUS"
if [[ -n $(git -C "$REPO" status --porcelain --untracked-files=no) ]]; then
  say "  AVISO: $REPO tiene cambios sin commitear; el worktree sale de $REV_ID y no los incluye"
fi

copiar_bins() { # destino
  mkdir -p "$1" || return 1
  cp "$TARGET/release/difuzz" "$1/difuzz" && cp "$TARGET/oraculo/difuzz" "$1/difuzz-oraculo"
}
compilar() { # etiqueta -> 0 si compilaron los dos perfiles
  local et=$1 p
  for p in release oraculo; do
    say "  cargo build --profile $p ($et)"
    if ! cargo build --profile "$p" --manifest-path "$WT/Cargo.toml" > "$LOGS/build-$et-$p.log" 2>&1; then
      say "  NO COMPILA ($p): ver $LOGS/build-$et-$p.log"
      rg -N -A6 '^error' "$LOGS/build-$et-$p.log" | rg -m 60 '' | tee -a "$LOG"
      return 1
    fi
  done
}

# --- 1 worktree -------------------------------------------------------------
if ! hecho worktree; then
  say "== 1 worktree"
  [[ -e $WT ]] && fallar "$WT ya existe y no hay marca: borrarlo a mano (git worktree remove)"
  git -C "$REPO" worktree add --detach "$WT" "$REV_ID" >> "$LOGS/worktree.log" 2>&1 || fallar "git worktree add (ver $LOGS/worktree.log)"
  marcar worktree
fi
# Despues del paso 11 el worktree ya no hace falta (se pueden rehacer los pasos 7-10 sin el).
hecho parche || [[ -d $WT ]] || fallar "no existe el worktree $WT (borrar $EST/worktree.ok para recrearlo)"

# --- 2 base -----------------------------------------------------------------
if ! hecho base; then
  say "== 2 base ($REV_ID tal cual)"
  if [[ -n ${BIN_BASE:-} ]]; then
    [[ -x $BIN_BASE && -x $BIN_BASE-oraculo ]] || fallar "BIN_BASE necesita $BIN_BASE y $BIN_BASE-oraculo ejecutables"
    mkdir -p "$OUT/bin-base" && cp "$BIN_BASE" "$OUT/bin-base/difuzz" && cp "$BIN_BASE-oraculo" "$OUT/bin-base/difuzz-oraculo" || fallar "copiar BIN_BASE"
    say "  binarios base copiados de $BIN_BASE"
  else
    [[ -z $(git -C "$WT" status --porcelain) ]] || fallar "el worktree ya tiene cambios: la base no seria $REV_ID"
    compilar base || fallar "la base $REV_ID no compila"
    copiar_bins "$OUT/bin-base" || fallar "copiar binarios base"
  fi
  marcar base
fi

# --- 3 subir ----------------------------------------------------------------
lock_pares() { rg -U -o -r '$1 $2' 'name = "([^"]+)"\nversion = "([^"]+)"' | sort; }
prefijo() { local l; while IFS= read -r l; do printf '%s %s\n' "$1" "$l"; done; }
# lock.diff sale de estado/lock-{antes,despues}.txt: se regenera en cada corrida (barato).
diff_lock() {
  [[ -f $EST/lock-antes.txt && -f $EST/lock-despues.txt ]] || return 0
  { echo "# Cargo.lock: - antes / + despues (name version)"
    comm -23 "$EST/lock-antes.txt" "$EST/lock-despues.txt" | prefijo -
    comm -13 "$EST/lock-antes.txt" "$EST/lock-despues.txt" | prefijo +; } > "$OUT/lock.diff"
  say "  Cargo.lock: $(rg -c '^- ' "$OUT/lock.diff" || echo 0) paquetes salen, $(rg -c '^\+ ' "$OUT/lock.diff" || echo 0) entran (ver $OUT/lock.diff)"
  local otros
  otros=$(rg '^[-+] ' "$OUT/lock.diff" | rg -v "^[-+] oxc[_-][^ ]+ (${VIEJA//./\\.}|${VERSION//./\\.})\$" || true)
  [[ -z $otros ]] || pendiente "Cargo.lock cambio paquetes fuera de la familia oxc $VIEJA: $(tr '\n' ' ' <<< "$otros")"
}
# La version vieja sale del Cargo.toml de REV (no del worktree, que el paso 3 cambia).
VIEJA=$(git -C "$REPO" show "$REV_ID:Cargo.toml" | rg -N -o -r '$1' '^oxc_minifier *= *"=([0-9][0-9.]*)"')
[[ -n $VIEJA ]] || fallar "no encuentro 'oxc_minifier = \"=X\"' en el Cargo.toml de $REV_ID"
[[ $VIEJA != "$VERSION" ]] || fallar "el Cargo.toml de $REV_ID ya esta en $VERSION"
if ! hecho subir; then
  # Idempotente: si un intento anterior cambio algo y fallo despues, se retoma sin deshacer.
  say "== 3 subir Cargo.toml"
  mapfile -t lineas < <(rg -N '^oxc_[a-z_]+ *= *"' "$WT/Cargo.toml")
  mapfile -t crates < <(printf '%s\n' "${lineas[@]}" | rg -o '^oxc_[a-z_]+')
  ((${#crates[@]})) || fallar "no hay crates oxc_* en Cargo.toml"
  for l in "${lineas[@]}"; do
    [[ $l =~ ^oxc_[a-z_]+\ *=\ *\"=(${VIEJA//./\\.}|${VERSION//./\\.})\"$ ]] || fallar "linea de oxc que no es \"=$VIEJA\" (todas tienen que ir juntas): $l"
  done
  say "  $VIEJA -> $VERSION en ${#crates[@]} crates: ${crates[*]}"
  git -C "$WT" show "$REV_ID:Cargo.lock" | lock_pares > "$EST/lock-antes.txt"
  sd "(?m)^(oxc_[a-z_]+ *= *)\"=${VIEJA//./\\.}\"" "\${1}\"=$VERSION\"" "$WT/Cargo.toml" || fallar "sd Cargo.toml"
  n=$(rg -c "^oxc_[a-z_]+ *= *\"=${VERSION//./\\.}\"" "$WT/Cargo.toml")
  [[ $n == "${#crates[@]}" ]] || fallar "Cargo.toml: $n lineas en $VERSION, se esperaban ${#crates[@]}"
  args=(); for c in "${crates[@]}"; do args+=(-p "$c"); done
  say "  cargo update ${args[*]}"
  cargo update --manifest-path "$WT/Cargo.toml" "${args[@]}" > "$LOGS/update.log" 2>&1 || {
    rg -N '' "$LOGS/update.log" | tee -a "$LOG"; fallar "cargo update (ver $LOGS/update.log)"; }
  lock_pares < "$WT/Cargo.lock" > "$EST/lock-despues.txt"
  # Ningun paquete que estaba en VIEJA puede quedar en VIEJA (la familia oxc se mueve junta).
  quedan=$(rg -N " ${VIEJA//./\\.}\$" "$EST/lock-despues.txt" | rg '^oxc' || true)
  [[ -z $quedan ]] || fallar "quedaron crates en $VIEJA en Cargo.lock: $quedan"
  for c in "${crates[@]}"; do
    rg -q "^$c ${VERSION//./\\.}\$" "$EST/lock-despues.txt" || fallar "Cargo.lock no tiene $c $VERSION"
  done
  # El string de version: Minifier::version() y los literales que lo repiten.
  mapfile -t archivos < <(rg -l -F "oxc_minifier $VIEJA" "$WT/src" "$WT/tests" 2>/dev/null | sort)
  for f in "${archivos[@]}"; do
    say "  ${f#"$WT"/}: $(rg -c -F "oxc_minifier $VIEJA" "$f") x \"oxc_minifier $VIEJA\" -> $VERSION"
    sd -F "oxc_minifier $VIEJA" "oxc_minifier $VERSION" "$f" || fallar "sd $f"
  done
  rg -q -F "\"oxc_minifier $VERSION\"" "$WT/src/backend/mod.rs" || fallar "Minifier::version() no quedo en $VERSION (src/backend/mod.rs)"
  # Lo demas que nombra la version vieja NO se cambia solo: documenta con que version se verifico
  # algo (comentarios, mensajes de tests). Se lista para leerlo a mano.
  rg -n -F "$VIEJA" "$WT/src" "$WT/tests" "$WT/Cargo.toml" 2>/dev/null | sd -F "$WT/" '' > "$OUT/menciones-vieja.txt"
  [[ -s $OUT/menciones-vieja.txt ]] && pendiente "$(rg -c '' "$OUT/menciones-vieja.txt") lineas de src/ y tests/ siguen nombrando $VIEJA (comentarios y mensajes; ver $OUT/menciones-vieja.txt)"
  # 9e: C1 (tests/muestra) exige oxc-minify de npm == Minifier::Oxc.version(): se mueven juntos.
  pj=$WT/probe/package.json
  if rg -q "\"oxc-minify\": *\"${VERSION//./\\.}\"" "$pj" 2>/dev/null; then
    say "  probe/package.json: oxc-minify ya en $VERSION"
  elif rg -q "\"oxc-minify\": *\"${VIEJA//./\\.}\"" "$pj" 2>/dev/null; then
    sd "(\"oxc-minify\": *)\"${VIEJA//./\\.}\"" "\${1}\"$VERSION\"" "$pj" || fallar "sd $pj"
    say "  probe/package.json: oxc-minify $VIEJA -> $VERSION"
  else
    pendiente "probe/package.json no fija oxc-minify $VIEJA: C1 (tests/muestra) exige la version del crate; revisarlo a mano"
  fi
  [[ $RED == 1 ]] || pendiente "probe/package-lock.json sigue en oxc-minify $VIEJA: correr con RED=1 (paso 10: npm install en probe/ y C1) o hacer npm install --save-exact oxc-minify@$VERSION en probe/ a mano"
  marcar subir
fi
diff_lock

# --- 4 compilar -------------------------------------------------------------
if ! hecho compilar; then
  say "== 4 compilar $VERSION"
  if ! compilar nuevo; then
    say ""
    say "NO COMPILA con oxc $VERSION (cambio de API?). Arreglar en $WT/src leyendo el fuente del crate en"
    say "  ~/.cargo/registry/src/*/oxc_*-$VERSION/ (nada de memoria), y volver a correr:"
    say "  $0 $VERSION $OUT"
    exit 3
  fi
  copiar_bins "$OUT/bin-nuevo" || fallar "copiar binarios nuevos"
  git -C "$WT" diff --stat -- src tests > "$OUT/api.stat"
  marcar compilar
fi

# --- 5 identidad ------------------------------------------------------------
if ! hecho identidad; then
  say "== 5 identidad de los binarios"
  for lado in base nuevo; do
    v=$VIEJA; [[ $lado == nuevo ]] && v=$VERSION
    r=$("$OUT/bin-$lado/difuzz" --preparar --info 2>&1 | rg -m1 '^difuzz-preparar') || r="(sin respuesta)"
    o=$("$OUT/bin-$lado/difuzz-oraculo" --preparar --info 2>&1 | rg -m1 '^difuzz-preparar') || o="(sin respuesta)"
    say "  $lado:    $r"; say "  $lado or: $o"
    [[ $r == *"oxc_minifier $v;"* && $r == *"debug_assertions=on" ]] || fallar "bin-$lado/difuzz no dice oxc_minifier $v con debug_assertions=on"
    [[ $o == "${r%=on}=off" ]] || fallar "bin-$lado/difuzz-oraculo no es el mismo difuzz sin debug assertions"
  done
  marcar identidad
fi

# --- 6 tests y clippy -------------------------------------------------------
if ! hecho tests; then
  say "== 6 cargo test y clippy (worktree)"
  cargo test --manifest-path "$WT/Cargo.toml" > "$LOGS/test.log" 2>&1; rc_t=$?
  rg -N '^test result:' "$LOGS/test.log" | tee -a "$LOG"
  if ((rc_t)); then
    pendiente "cargo test falla (rc $rc_t): $(rg -N -o '^test (\S+) \.\.\. FAILED' -r '$1' "$LOGS/test.log" | tr '\n' ' ')(ver $LOGS/test.log)"
  fi
  cargo clippy --all-targets --manifest-path "$WT/Cargo.toml" -- -D warnings > "$LOGS/clippy.log" 2>&1; rc_c=$?
  if ((rc_c)); then pendiente "clippy --all-targets -D warnings falla (rc $rc_c): ver $LOGS/clippy.log"; else say "  clippy limpio"; fi
  echo "test=$rc_t clippy=$rc_c" > "$EST/tests.rc"
  marcar tests
fi

# --- 7 golden ---------------------------------------------------------------
# Tabla de verificar.sh -> "caso|celda<TAB>ok X | MAL X!=Y" (cada celda son dos palabras).
golden_tabla() {
  local -a cel w; local l i
  while IFS= read -r l; do
    read -r -a w <<< "$l"
    if [[ ${w[0]:-} == caso ]]; then cel=("${w[@]:1}"); continue; fi
    ((${#cel[@]})) && ((${#w[@]} == 2 * ${#cel[@]} + 1)) || continue
    for ((i = 0; i < ${#cel[@]}; i++)); do printf '%s|%s\t%s %s\n' "${w[0]}" "${cel[i]}" "${w[2*i+1]}" "${w[2*i+2]}"; done
  done < "$1"
}
if ! hecho golden; then
  # El golden (tests/golden) y la aceptacion de los re-chequeos (tests/aceptacion/rechequeos) usan
  # el mismo verificar.sh y la misma tabla de salida.
  for suite in golden aceptacion; do
    dir=$GOLDEN; [[ $suite == aceptacion ]] && dir=$ACEPTACION
    if [[ $dir == no ]]; then say "== 7 $suite: salteado (${suite^^}=no)"; continue; fi
    say "== 7 $suite ($dir)"
    [[ -x $dir/verificar.sh ]] || fallar "no encuentro $dir/verificar.sh (${suite^^}=no para saltearlo)"
    so=$OUT/$suite
    for lado in base nuevo; do
      rm -rf "${so:?}/$lado"; mkdir -p "$so"
      "$dir/verificar.sh" "$OUT/bin-$lado/difuzz" "$so/$lado" > "$so/$lado.log" 2>&1
      rc=$?
      say "  $lado: rc=$rc $(rg -N -o 'comparaciones=[0-9]+ fallas=[0-9]+' "$so/$lado.log")"
    done
    golden_tabla "$so/base.log" | sort > "$TMPDIR/gb"; golden_tabla "$so/nuevo.log" | sort > "$TMPDIR/gn"
    # Lineas de modos y de stats (no son de la tabla).
    rg -N '^(MAL|modo|celda) ' "$so/base.log" | sort > "$TMPDIR/gmb"
    rg -N '^(MAL|modo|celda) ' "$so/nuevo.log" | sort > "$TMPDIR/gmn"
    {
      echo "# $suite: celdas cuyo resultado cambia de base a nuevo (caso|celda, base, nuevo)"
      join -t $'\t' -a1 -a2 -e '<falta>' -o 0,1.2,2.2 "$TMPDIR/gb" "$TMPDIR/gn" | while IFS=$'\t' read -r k b n; do
        [[ $b == "$n" ]] || printf '%s\t%s\t%s\n' "$k" "$b" "$n"; done
      diff "$TMPDIR/gmb" "$TMPDIR/gmn" | rg '^[<>]' | sd '^<' 'base:' | sd '^>' 'nuevo:'
    } > "$so/cambios.tsv"
    (($(rg -c '' "$TMPDIR/gb" || echo 0) > 0)) || pendiente "$suite: no pude leer la tabla de $so/base.log"
    nc=$(rg -c -v '^#' "$so/cambios.tsv" || echo 0)
    if ((nc)); then
      pendiente "el $suite cambia en $nc celdas/lineas ($so/cambios.tsv). Si es un fix de oxc confirmado, hay que actualizar $dir/esperado.tsv (y su README) A MANO; si no, es una regresion"
      rg -N -v '^#' "$so/cambios.tsv" | tee -a "$LOG"
    else
      say "  $suite sin cambios entre base y nuevo"
    fi
  done
  marcar golden
fi

# --- 8 arnes ----------------------------------------------------------------
IFS=';' read -r -a celdas <<< "$CELDAS"
if ! hecho arnes; then
  say "== 8 arnes de no regresion ($ARNES)"
  [[ -f $ARNES ]] || fallar "no encuentro el arnes: $ARNES"
  # ARNES_ARGS sin comillas a proposito: se parte en palabras.
  # shellcheck disable=SC2086
  bash "$ARNES" --jobs "$JOBS" --corpus "$CORPUS" --ignorar-linea '^backend: oxc_minifier ' $ARNES_ARGS \
    "$OUT/bin-base/difuzz" "$OUT/bin-nuevo/difuzz" "$OUT/arnes" "${celdas[@]}" > "$LOGS/arnes.log" 2>&1
  rc=$?
  say "  arnes rc=$rc: $(rg -N '^RESULTADO' "$OUT/arnes/DIFERENCIAS.txt" 2>/dev/null)"
  ((rc <= 1)) || fallar "el arnes no pudo comparar (rc $rc): ver $OUT/arnes/DIFERENCIAS.txt"
  marcar arnes
fi

# --- 9 informe de diferencias -----------------------------------------------
DESTINOS=(div known panic asserts lint recursos)
# raiz de la celda dentro de un --out del arnes (un solo directorio, salvo cache-a)
raiz() { local d; mapfile -t d < <(eza -1 --only-dirs "$1" 2>/dev/null | rg -v '^(cache-a)?$'); ((${#d[@]} == 1)) && printf '%s\n' "$1/${d[0]}"; }
# indice de una celda: "caso<TAB>estado<TAB>dir" (estado: div:<kind>, known:<kind>, panic, ..., descarte:<causa>)
indice() {
  local r=$1 d dir caso kind
  for d in "${DESTINOS[@]}"; do
    [[ -d $r/$d ]] || continue
    while IFS= read -r dir; do
      [[ -n $dir ]] || continue
      caso=$(rg -N -m1 -o -r '$1' '^case: (.*)$' "$r/$d/$dir/meta.txt" 2>/dev/null) || caso="?$dir"
      kind=$(rg -N -m1 -o -r '$1' '^kind: (.*)$' "$r/$d/$dir/meta.txt" 2>/dev/null) || kind=""
      case $d in div|known) printf '%s\t%s:%s\t%s\n' "$caso" "$d" "$kind" "$r/$d/$dir";;
                 *) printf '%s\t%s\t%s\n' "$caso" "$d" "$r/$d/$dir";; esac
    done < <(eza -1 --only-dirs "$r/$d")
  done
  [[ -f $r/descartes.tsv ]] && rg -N -v '^#' "$r/descartes.tsv" | cut -f1,2 | while IFS=$'\t' read -r caso kind; do
    printf '%s\tdescarte:%s\t-\n' "$caso" "$kind"; done
}
hallazgo() { [[ $1 == *div:* || $1 == *panic* ]]; }
stats_diff() { # stdout_base stdout_nuevo -> "  stats: clave base -> nuevo, ..." de la linea total=
  local -a tb tn; local i k s=""
  read -r -a tb < <(rg -N -m1 '^total=' "$1" | tr -d '()|'); read -r -a tn < <(rg -N -m1 '^total=' "$2" | tr -d '()|')
  if ((${#tb[@]} != ${#tn[@]})); then printf '  stats: formatos distintos (ver los .stdout del arnes)\n'; return; fi
  for ((i = 0; i < ${#tb[@]}; i++)); do
    [[ ${tb[i]} == "${tn[i]}" ]] && continue
    k=${tb[i]%%=*}; s+=" $k ${tb[i]#*=}->${tn[i]#*=}"
  done
  printf '  stats:%s\n' "${s:- sin cambios}"
}
# Casos con "DIF contenido de" o "DIF meta.txt de" en DIFERENCIAS.txt: "etiqueta<TAB>destino/caso".
contenido_por_celda() {
  local l et=""
  while IFS= read -r l; do
    if [[ $l =~ ^==\ celda\ .*\(([^\)]+)\)$ ]]; then et=${BASH_REMATCH[1]}
    elif [[ -n $et && $l =~ ^\ \ DIF\ (contenido|meta\.txt)\ de\ ([^/]+/[^/]+) ]]; then printf '%s\t%s\n' "$et" "${BASH_REMATCH[2]}"; fi
  done < "$OUT/arnes/DIFERENCIAS.txt" | sort -u
}
minificado() { # bin caso min conf forma carril -> el minificado que imprime --emitir (o ERROR rc)
  local s rc
  s=$("$1" --emitir "$CORPUS/$2" --minifier "$3" --config "$4" --forma "$5" --carril "$6" 2>&1); rc=$?
  ((rc)) && { printf 'ERROR rc=%s\n%s\n' "$rc" "$s"; return; }
  printf '%s\n' "$s" | rg -N -A100000 '^// ---- minificado$' | rg -v '^// ---- minificado$'
}
if ! hecho informe; then
  say "== 9 informe de diferencias"
  tsv=$OUT/diferencias.tsv
  printf 'celda\tcaso\tbase\tnuevo\tcategoria\tminificado\tpista\tdir_base\tdir_nuevo\n' > "$tsv"
  rm -rf "$OUT/triage"
  for celda in "${celdas[@]}"; do
    et=${celda// /-}; et=${et//---/-}; et=${et//--/-}; et=${et//[^A-Za-z0-9_.-]/_}   # = etiqueta() del arnes
    rb=$(raiz "$OUT/arnes/base/$et") && rn=$(raiz "$OUT/arnes/nuevo/$et") || { pendiente "informe: no encuentro la celda $et en $OUT/arnes"; continue; }
    # <minifier>+<config>+<forma>[+carril_b][+invocar]...: solo los tres primeros
    IFS=+ read -r cmin cconf cforma _ <<< "${rb##*/}"
    read -r -a w <<< "$celda"; ccarril=${w[1]}
    declare -A eb=() en=() db=() dn=()
    while IFS=$'\t' read -r c e d; do eb[$c]+="${eb[$c]:++}$e"; [[ $d != - ]] && db[$c]=$d; done < <(indice "$rb" | sort)
    while IFS=$'\t' read -r c e d; do en[$c]+="${en[$c]:++}$e"; [[ $d != - ]] && dn[$c]=$d; done < <(indice "$rn" | sort)
    n_des=0; n_apa=0; n_cam=0
    while IFS= read -r c; do
      b=${eb[$c]:-igual}; n=${en[$c]:-igual}
      [[ $b == "$n" ]] && continue
      if hallazgo "$b" && ! hallazgo "$n"; then cat=DESAPARECE; n_des=$((n_des + 1))
      elif ! hallazgo "$b" && hallazgo "$n"; then cat=APARECE; n_apa=$((n_apa + 1))
      else cat=CAMBIA; n_cam=$((n_cam + 1)); fi
      # El minificado de los dos binarios: si es identico, el cambio no viene de oxc.
      tdir=$OUT/triage/$et/${c//\//__}; mkdir -p "$tdir"
      minificado "$OUT/bin-base/difuzz" "$c" "$cmin" "$cconf" "$cforma" "$ccarril" > "$tdir/base.min.js"
      minificado "$OUT/bin-nuevo/difuzz" "$c" "$cmin" "$cconf" "$cforma" "$ccarril" > "$tdir/nuevo.min.js"
      # Si --emitir falla (no parsea, panic) no hay minificado que comparar: "identico" no diria nada.
      eb_=0; en_=0
      rg -q '^ERROR rc=' "$tdir/base.min.js" && eb_=1
      rg -q '^ERROR rc=' "$tdir/nuevo.min.js" && en_=1
      if ((eb_ || en_)); then mn="sin_minificado($( ((eb_)) && printf base)$( ((eb_ && en_)) && printf +)$( ((en_)) && printf nuevo))"
      elif diff -q "$tdir/base.min.js" "$tdir/nuevo.min.js" >/dev/null; then mn=identico
      else mn=cambio; diff -u --label base --label nuevo "$tdir/base.min.js" "$tdir/nuevo.min.js" > "$tdir/min.diff"; fi
      if [[ $b == descarte:* && $n == descarte:* ]]; then pista="sigue descartado, cambia la causa: el parser o el analisis de oxc cambio de criterio (no se pierde ni gana un hallazgo)"
      elif [[ $mn == identico ]]; then pista="minificado identico: no viene de oxc (no determinismo o entorno)"
      elif [[ $mn == sin_minificado* ]]; then pista="--emitir falla ($mn): ver triage/ y el meta.txt del caso"
      elif [[ $cat == DESAPARECE && $n == igual ]]; then pista="oxc cambio el minificado y ya no diverge: posible fix, confirmar (commit/PR de oxc, npm)"
      elif [[ $cat == DESAPARECE && $n == *descarte:* ]]; then pista="difuzz ya no lo compara (descarte): revisar, puede esconder el bug"
      elif [[ $cat == DESAPARECE && $n == *known* ]]; then pista="ahora lo explica una asuncion: revisar que el sensor tenga razon"
      elif [[ $cat == DESAPARECE ]]; then pista="cambia a $n: revisar"
      elif [[ $cat == APARECE ]]; then pista="hallazgo nuevo: triagear (bug nuevo o regresion de oxc)"
      else pista="cambia de destino o kind: revisar"; fi
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$et" "$c" "$b" "$n" "$cat" "$mn" "$pista" "${db[$c]:--}" "${dn[$c]:--}" >> "$tsv"
    done < <(printf '%s\n' "${!eb[@]}" "${!en[@]}" | sort -u)
    # Mismo estado pero otro contenido (minificado, salidas): lo detalla el arnes, aca solo el numero.
    ncont=$(contenido_por_celda | rg -c "^$et"$'\t' || echo 0)
    say "  $et: desaparecen=$n_des aparecen=$n_apa cambian=$n_cam  (mismo destino con otro contenido: $ncont casos)"
    unset eb en db dn
  done
  {
    echo "subir-oxc $VIEJA -> $VERSION (REV $REV_ID), $(date -u +%FT%TZ)"
    echo
    echo "Hallazgos = div/ y panic/. DESAPARECE: estaba y ya no (pasa a igual, descarte, known...)."
    echo "APARECE: no estaba y ahora si. CAMBIA: otro destino o kind sin entrar ni salir de div/panic."
    echo "minificado: --emitir del caso con cada binario: identico (la diferencia no la causa oxc), cambio, o"
    echo "sin_minificado(base|nuevo) si --emitir falla en ese lado (no parsea, panic)."
    echo
    for celda in "${celdas[@]}"; do
      et=${celda// /-}; et=${et//---/-}; et=${et//--/-}; et=${et//[^A-Za-z0-9_.-]/_}
      echo "== $et"
      s=$(rg -N "^$et"$'\t' "$tsv" | cut -f2-7 | while IFS= read -r l; do printf '  %s\n' "${l//$'\t'/ | }"; done)
      printf '%s\n' "${s:-  (ningun caso cambia de estado)}"
      # Contadores de la linea de stats que cambian (por posicion: hay claves repetidas, p. ej. timeout).
      stats_diff "$OUT/arnes/base/$et.stdout" "$OUT/arnes/nuevo/$et.stdout"
      printf '  mismo destino con otro contenido (meta.txt o archivos; detalle en arnes/DIFERENCIAS.txt): %s\n' \
        "$(contenido_por_celda | rg "^$et"$'\t' | cut -f2 | tr '\n' ' ')"
    done
  } > "$OUT/INFORME.txt"
  marcar informe
fi

# --- 10 npm (opcional) ------------------------------------------------------
if [[ $RED == 1 ]] && ! hecho npm; then
  say "== 10 npm oxc-minify@$VERSION y repro de ENVIAR.md ($REPRO)"
  command -v npm >/dev/null || fallar "RED=1 sin npm"
  mkdir -p "$OUT/npm/repro"
  [[ -f $OUT/npm/package.json ]] || printf '{ "name": "probe-oxc-%s", "private": true }\n' "$VERSION" > "$OUT/npm/package.json"
  npm install --prefix "$OUT/npm" --no-audit --no-fund "oxc-minify@$VERSION" > "$LOGS/npm.log" 2>&1 || fallar "npm install (ver $LOGS/npm.log)"
  for f in "$REPRO"/*.mjs; do
    [[ $f == *.out.mjs ]] && continue
    b=$(basename "$f" .mjs); cp "$f" "$OUT/npm/repro/"
    node "$OUT/npm/repro/$b.mjs" > "$OUT/npm/repro/$b.out" 2>&1
    if [[ -f $REPRO/$b.out ]] && diff -q "$REPRO/$b.out" "$OUT/npm/repro/$b.out" >/dev/null; then
      say "  $b: igual que el .out de ENVIAR"
    else
      pendiente "repro $b de ENVIAR.md da otra salida con oxc-minify@$VERSION (diff $REPRO/$b.out $OUT/npm/repro/$b.out): el bug pudo cambiar o arreglarse"
    fi
  done
  # 9e: probe/ del worktree con oxc-minify@VERSION (package.json y package-lock.json) y C1 de la
  # muestra (tests/muestra/auditoria.rs: el crate imprime lo mismo que oxc-minify de npm).
  if [[ -d $WT ]]; then
    npm install --prefix "$WT/probe" --save-exact --no-audit --no-fund "oxc-minify@$VERSION" > "$LOGS/npm-probe.log" 2>&1 \
      || fallar "npm install en probe/ (ver $LOGS/npm-probe.log)"
    cargo test --manifest-path "$WT/Cargo.toml" c1_igual_a_oxc_minify_de_npm -- --ignored > "$LOGS/c1.log" 2>&1; rc=$?
    rg -N '^test result:' "$LOGS/c1.log" | tee -a "$LOG"
    if ((rc)); then pendiente "C1 (oxc-minify@$VERSION de npm contra el crate, tests/muestra) falla (rc $rc): ver $LOGS/c1.log"; else say "  C1 verde"; fi
  else
    pendiente "sin worktree ($WT): no pude subir oxc-minify en probe/ ni correr C1 (volver a correr con MANTENER=1 desde el paso 10)"
  fi
  # Las asunciones de oxc (tablas de backend/oxc.rs) salen de este archivo, que NO viaja en el
  # crate publicado: se compara su blob entre los tags del repo de oxc.
  if command -v gh >/dev/null; then
    a=$(gh api "repos/oxc-project/oxc/contents/crates/oxc_minifier/docs/ASSUMPTIONS.md?ref=crates_v$VIEJA" --jq .sha 2>>"$LOGS/gh.log")
    b=$(gh api "repos/oxc-project/oxc/contents/crates/oxc_minifier/docs/ASSUMPTIONS.md?ref=crates_v$VERSION" --jq .sha 2>>"$LOGS/gh.log")
    if [[ -z $a || -z $b ]]; then pendiente "no pude leer ASSUMPTIONS.md de los tags crates_v$VIEJA / crates_v$VERSION (ver $LOGS/gh.log)"
    elif [[ $a == "$b" ]]; then say "  ASSUMPTIONS.md igual entre crates_v$VIEJA y crates_v$VERSION (blob ${a:0:8})"
    else pendiente "ASSUMPTIONS.md cambio entre crates_v$VIEJA y crates_v$VERSION: revisar las tablas de asunciones de src/backend/oxc.rs"; fi
  else
    pendiente "sin gh: comparar a mano crates/oxc_minifier/docs/ASSUMPTIONS.md entre crates_v$VIEJA y crates_v$VERSION"
  fi
  marcar npm
fi

# --- avisos -----------------------------------------------------------------
if [[ -f $ENVIAR ]]; then
  nv=$(rg -c -F "$VIEJA" "$ENVIAR" || echo 0)
  ((nv)) && pendiente "ENVIAR.md nombra $VIEJA $nv veces (versiones y comandos npm i). Antes de mandar: reproducir cada texto con $VERSION (RED=1) y actualizar la version; un hallazgo que DESAPARECE puede ser uno de esos issues"
fi

# --- 11 parche y limpieza ---------------------------------------------------
if ! hecho parche; then
  say "== 11 parche"
  git -C "$WT" diff > "$OUT/subida-oxc-$VERSION.patch" || fallar "git diff"
  git -C "$WT" diff --stat | tee -a "$LOG"
  nuevos=$(git -C "$WT" status --porcelain | rg '^\?\?' || true)
  [[ -z $nuevos ]] || pendiente "archivos nuevos en el worktree que el parche NO lleva: $nuevos"
  marcar parche
fi
if [[ -d $WT && $MANTENER != 1 ]]; then
  git -C "$REPO" worktree remove --force "$WT" && say "  worktree $WT borrado (el parche lo guarda)"
fi
rm -rf "$TMPDIR"

say ""
mapfile -t revisar < <(rg -N '' "$EST/revisar.txt" 2>/dev/null)
if ((${#revisar[@]})); then
  say "TERMINADO $VIEJA -> $VERSION con ${#revisar[@]} cosas para revisar ($EST/revisar.txt):"
  for r in "${revisar[@]}"; do say "  - $r"; done
  say "informe: $OUT/INFORME.txt  tabla: $OUT/diferencias.tsv  parche: $OUT/subida-oxc-$VERSION.patch"
  exit 1
fi
say "TERMINADO $VIEJA -> $VERSION: verde y sin diferencias en hallazgos. Parche: $OUT/subida-oxc-$VERSION.patch"
exit 0
