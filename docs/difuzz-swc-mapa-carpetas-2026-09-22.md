# difuzz (SWC) — Mapa de carpetas · 22-sep-2026

> **Actualizado el 23-sep: ver §7.** Donde §7 contradice a §1–§6, vale §7.

**Qué es este doc:** qué es cada carpeta del trabajo difuzz/SWC, de qué depende, y si se
puede tocar o borrar. Todo es específico de difuzz + SWC.

**Cómo leerlo:** lo marcado **VERIFICADO** salió de salida de comandos en las sesiones del
21 y 22-sep (incluida la noche del 22, que corrigió este doc). Lo marcado **SUPUESTO** no se comprobó: el script del §5 lo confirma o lo
corrige. Antes de actuar sobre una carpeta, correr el script.

---

## 1. Vista rápida

```
~/difuzz                         ← LA HERRAMIENTA (git, fuente de verdad)
~/difuzz-main                    ← copia vieja de difuzz, apunta a ~/swc-main (SIN git)
~/difuzz-main2                   ← difuzz nuevo contra ~/swc-main: el que mide main
~/difuzz-out-4                   ← BASE DEL 22-sep (9 divergencias)
~/difuzz-out-rv                  ← corrida intermedia de la madrugada del 22 (11)
~/difuzz-bisect                  ← descartable

~/Desktop/OpenSource/swc         ← CLON PRINCIPAL de SWC (tiene el .git real)
~/swc-main                       ← worktree en upstream/main (lo usan difuzz-main y -main2)
~/swc-12400                      ← worktree de la PR #12401

/tmp/...                         ← volátil, se pierde al reiniciar
~/Downloads/...                  ← parches, docs y scripts sueltos

~/Desktop/OpenSource/dapr        ← OTRO proyecto, fuera de este mapa
```

---

## 2. Carpetas de difuzz

| carpeta | qué es | git | SWC contra el que compila | ¿se toca? |
|---|---|---|---|---|
| `~/difuzz` | la herramienta actual: `paren_remover`, `--jobs`, `known.rs`, reporter, probe | **sí** · `master` @ `59554e7`, remoto `baltasarblanco/difuzz` · VERIFICADO | crates.io, `swc_ecma_minifier` 61.0.8 · VERIFICADO (doc que-sigue) | sí, es donde se desarrolla |
| `~/difuzz/probe` | scripts de Node contra `@swc/core` | dentro de `~/difuzz` | `@swc/core` 1.16.2 · SUPUESTO (el probe de 8953 corrió con esa versión desde ahí) | sí |
| `~/difuzz/difuzz-out` | salida por defecto de `~/difuzz`. Hoy solo tiene `div/`: **34 entradas del 21-sep 10:54**, corrida vieja anterior a `known.rs` · VERIFICADO. **No es la base del 22-sep** | ignorado o no: SUPUESTO | — | no a mano. Que el reporter la limpie al arrancar: SUPUESTO sin verificar. Si es cierto, correr `~/difuzz` sin `--out` la pisa |
| `~/difuzz/docs` | que-sigue, lecciones, estado | versionado | — | solo con commit |
| `~/difuzz-main` | copia scratch **vieja**: sin `known.rs`, 4 archivos distintos, sin `--jobs`, `<corpus_dir>` posicional · VERIFICADO | **no es repo git** · VERIFICADO | `[patch.crates-io]` → 6 crates en `/home/baltasarblanco/swc-main/crates/…` · VERIFICADO | **no**: sin git para volver atrás. Reproduce la base del 21-sep **solo** con `~/swc-main` vuelto a `4b42cd59d7` (hoy está en `42e4f7e98b`) |
| `~/difuzz-main2` | código de `~/difuzz` (`src/`, `Cargo.toml`, `Cargo.lock`) + el `[patch.crates-io]` de `~/difuzz-main`. Adentro: `corpus-4b42/` (copia fija de `tests/` @ `4b42cd59d7`, 3119 `input.js`, 118M), `out-4b42/` (9), `out-main/` (8), `build*.log`, `run*.log` · VERIFICADO | no | `~/swc-main` · VERIFICADO con `cargo tree` (también arrastra `swc_ecma_transforms_optimization` por ruta) | sí: es el binario para medir `main`. Si `~/difuzz/src` cambia, re-copiar |
| `~/difuzz-out-4` | `div/` (9), `known/`, `preload.cjs`; `div/` del 22-sep 00:41 · VERIFICADO | no | binario nuevo; SWC y árbol del corpus (3102 casos): SUPUESTO | **no**: es la base del 22-sep |
| `~/difuzz-out-rv` | `div/` (11), `known/`, `preload.cjs`; 22-sep 00:21. Tiene `delete_conditional_1/2` y `delete_binary_1/2`; le faltan `duplicate_argname` e `issue_2101` · VERIFICADO | no | binario intermedio: SUPUESTO | descartable; no se usa como base |
| `~/difuzz-bisect` | bisect de una sesión anterior | SUPUESTO | SUPUESTO | descartable según el prompt de sesión; confirmar con el script antes de borrar |

**Las dos bases de comparación, que no se mezclan:**

| base | binario | SWC | corpus | divergencias | dónde vive |
|---|---|---|---|---|---|
| 21-sep | `~/difuzz-main` (viejo) | `~/swc-main` @ `4b42cd59d7` | 3119 | 19 | se reproduce corriendo `~/difuzz-main` con `~/swc-main` en `4b42cd59d7` |
| 22-sep | `~/difuzz` (nuevo) | crates.io 61.0.8 (SUPUESTO) | 3102, árbol NO SE | 9 | `~/difuzz-out-4/div` |
| A/B antes | `~/difuzz-main2` | `4b42cd59d7` | `corpus-4b42` (3119) | 9 (mismas que 22-sep) | `~/difuzz-main2/out-4b42/div` |
| A/B después | `~/difuzz-main2` | `42e4f7e98b` | `corpus-4b42` (3119) | 8 (sale `11084`) | `~/difuzz-main2/out-main/div` |

**Para medir `main` de ahora en adelante:** `~/difuzz-main2` sobre `corpus-4b42`, contra
`out-main`. Mismo binario y mismo corpus; lo único que cambia es SWC. No comparar contra
`~/difuzz-out-4`: cruza SWC y corpus distintos.

---

## 3. Carpetas de SWC

| carpeta | qué es | rama / commit | ¿se toca? |
|---|---|---|---|
| `~/Desktop/OpenSource/swc` | **clon principal**. Tiene el `.git` real; los worktrees guardan su metadata en `.git/worktrees/`. Remotos: `origin` = fork `baltasarblanco/swc`, `upstream` = `swc-project/swc` · VERIFICADO (push de #12401 fue a `baltasarblanco/swc.git`) | `fix/minifier-for-if-else-break` @ `05a104d8f6` · VERIFICADO | **no** cambiar de rama ni commitear. `git fetch` sí |
| `~/swc-main` | **worktree** del clon principal en `upstream/main`, detached, limpio · VERIFICADO (`git worktree list`). Lo usan `~/difuzz-main` y `~/difuzz-main2` por `[patch]` | `42e4f7e98b` · VERIFICADO (movido desde `4b42cd59d7` la noche del 22-sep) | sí, para actualizarlo: `git -C ~/swc-main fetch upstream && git -C ~/swc-main switch --detach upstream/main`. Moverlo cambia lo que miden **las dos** copias de difuzz |
| `~/swc-12400` | worktree de la PR **#12401** | `fix/12400-dce-self-ref-usage` @ `364447291a` · VERIFICADO | solo para cambios de review de #12401. **Borrar después del merge** con `git worktree remove ~/swc-12400` |

**Regla:** los worktrees se borran con `git worktree remove`, nunca con `rm -rf`, porque
dejan metadata huérfana en el clon principal.

---

## 4. Archivos sueltos

**`/tmp`** (se pierde al reiniciar; nada de acá es fuente de verdad):

| archivo | qué es |
|---|---|
| `/tmp/Cargo.toml.bak` | backup del `Cargo.toml` de `~/difuzz-main` apuntando a `~/swc-main` |
| `/tmp/difuzz-12400/` | corrida de difuzz viejo contra el fix de #12401 (18 divergencias) |
| `/tmp/12400.log` | log de `RUST_LOG=debug` del repro de #12400 |
| `/tmp/pr-12400.md`, `/tmp/issue-8953.md` | bodies de #12401 y #12404 ya publicados |

**`~/Downloads`** (candidatos a limpiar; lo que importa ya está commiteado o publicado):

| archivo | estado |
|---|---|
| `0001-difuzz-Config-NAME-*.patch`, `0001-docs-que-sigue-*.patch` | aplicados (`4914b6e`, `c108afb`) |
| `12400-dce.diff`, `12400-tests.txt` | aplicados en `364447291a` |
| `probe-8953.mjs` | probe de #12404; si se quiere conservar, moverlo a `~/difuzz/probe/` |
| `difuzz-swc-que-sigue-2026-09-22.md` | versión **vieja** de la mañana. Borrar para no confundir |
| `difuzz-swc-que-sigue-2026-09-22 (1).md` | versión de cierre, ya commiteada en `59554e7` |
| `lecciones-pr-12401-swc.md` | ya commiteada en `5ce6fd0` |

---

## 5. Script de verificación

Solo lectura. Confirma o corrige cada SUPUESTO de este doc.

```bash
for d in ~/difuzz ~/difuzz-main ~/difuzz-main2 ~/difuzz-bisect \
         ~/Desktop/OpenSource/swc ~/swc-main ~/swc-12400; do
  echo "=== $d"
  [ -d "$d" ] || { echo "  no existe"; continue; }
  if git -C "$d" rev-parse --git-dir >/dev/null 2>&1; then
    echo "  git: $(git -C "$d" rev-parse --abbrev-ref HEAD) @ $(git -C "$d" rev-parse --short HEAD)"
    echo "  toplevel: $(git -C "$d" rev-parse --show-toplevel)  git-dir: $(git -C "$d" rev-parse --git-dir)"
    echo "  sucio: $(git -C "$d" status --short | wc -l) archivos"
  else
    echo "  git: NO"
  fi
  [ -f "$d/Cargo.toml" ] && grep -E "path *= *\"" "$d/Cargo.toml" | sed 's/^/  patch: /' | head -8
  echo "  tamaño: $(du -sh "$d" 2>/dev/null | cut -f1)"
done
echo "=== worktrees del clon principal"
git -C ~/Desktop/OpenSource/swc worktree list
echo "=== bases (22-sep y A/B)"
for b in ~/difuzz-out-4 ~/difuzz-main2/out-4b42 ~/difuzz-main2/out-main; do
  echo "  $b: $(ls "$b/div" 2>/dev/null | wc -l)"
done
echo "=== @swc/core del probe"
node -e "console.log(require('$HOME/difuzz/probe/node_modules/@swc/core/package.json').version)" 2>/dev/null || echo "no está en ~/difuzz/probe"
```

Qué mirar en la salida:
- `worktree list` dice si `~/swc-main` es worktree del clon principal o un clon aparte.
- `git-dir` de un worktree apunta a `.../swc/.git/worktrees/<nombre>`.
- `du` muestra el peso real; los `target/` de Rust suelen ser lo que más ocupa.
- Bases: `~/difuzz-out-4` 9, `out-4b42` 9, `out-main` 8.

---

## 6. Qué se puede borrar, y cuándo

| qué | cuándo |
|---|---|
| `~/swc-12400` | después del merge o cierre de #12401, con `git worktree remove` |
| `~/difuzz-bisect` | cuando el script confirme que no tiene cambios sin commitear que importen |
| `~/difuzz-main` | **nunca mientras la base del 21-sep sea útil**. Si se quiere liberar espacio: `cargo clean` adentro (borra `target/`, conserva el código) |
| `~/difuzz-out-rv`, `~/difuzz/difuzz-out` (34, 21-sep) | cuando se decida; no son base de nada |
| `~/difuzz-out-4` | no mientras haga falta cotejar contra el 22-sep |
| `target/` de cualquier copia de difuzz | cuando haga falta espacio; se regenera con `cargo build` |
| `/tmp/...` | solo; al reiniciar |
| `~/Downloads/...` de §4 | ya |

---

*Complementa a `difuzz-swc-que-sigue-2026-09-22.md` (versión de cierre) y
`difuzz-swc-estado-2026-09-21.md`. Actualizar cuando se borre `~/swc-12400`, se mueva
`~/swc-main` o se re-copie `~/difuzz-main2`.*

---

## 7. Actualización del 23-sep (serie de mejoras)

Todo VERIFICADO con salida de comandos del 23-sep salvo lo marcado.

**Cambios de estado**

| carpeta | ahora |
|---|---|
| `~/swc-main` | `42e4f7e98b`, detached, limpio. Estuvo un rato en `4b42cd59d7` para validar el lint y volvió |
| `~/difuzz` | `master` con 10 commits locales sobre `9ac4db5` (c1–c8, m6, m6b) + m6c y docs pendientes de aplicar. **Sin push.** Nuevos: `src/checks.rs`, `scripts/` |
| `~/difuzz-main2` | `src/` = último commit de `~/difuzz` extraído con `git archive` (+ `touch`). `Cargo.toml` = el de `~/difuzz` + `[patch.crates-io]` a `~/swc-main`, **incluido `swc_ecma_visit`**. `Cargo.toml.antes-serie` = el previo a la serie |

**Salidas nuevas en `~/difuzz-main2`** (todas contra `corpus-4b42`)

| carpeta | qué es |
|---|---|
| `out-c1` … `out-c6` | medición de la serie de confianza (`scripts/medir-serie.sh`, corrida válida) |
| `out-c7`, `out-c8` | commits 7 y 8, medidos a mano |
| `out-lint-4b42` | c8 contra `4b42cd59d7`: validación del lint (9739 en `lint/`, 11084 en `div/`) |
| `out-matriz/` | matriz m6: una carpeta por celda + `<celda>.stats` y `<celda>.log` |
| `out-matriz-b/` | matriz m6b (verificación de nombres, con el error que tapaba 12188) |
| `out-matriz-c/` | PENDIENTE: matriz m6c |
| `build-*.log`, `run-*.log`, `stats-c*.txt` | logs de cada build y corrida |

**Bases de comparación vigentes**

| base | dónde | para qué |
|---|---|---|
| `out-main` | `~/difuzz-main2/out-main` | la base previa a la serie (8 div) |
| c8 | `~/difuzz-main2/out-c8` = `out-matriz/carril_a+tal_cual` | estado del eje de confianza |
| matriz | `~/difuzz-main2/out-matriz-c/` (cuando exista) | estado del eje de volumen |

**Nuevo en cada `out-*`:** `lint/` (lo borra el reporter al arrancar, como `div/`, `known/` y
`panic/`, VERIFICADO en `oracle.rs`) y `known-sintomas.tsv` (**no** se borra: persiste entre
corridas del mismo directorio; su encabezado lleva el id del oráculo).

**Temporales:** `/tmp/ukhd/` (corpus de un caso para `unused_keep_harmony`, que resultó ser el
caso de #12401), `/tmp/ukhd-out/`.

**Scripts** (antes en `~/Downloads`, ahora versionados en `~/difuzz/scripts/`): `matriz.sh`
(reutilizable: `bash scripts/matriz.sh <dir_salida>`), `medir-serie.sh` (mide `HEAD~5..HEAD`;
específico de la serie de confianza).

**Qué se puede borrar:** `out-c1` … `out-c7` y `out-matriz-b` cuando la documentación esté
subida (la historia queda en los docs). `out-main`, `out-c8`, `out-lint-4b42` y la última
matriz se conservan como bases.
