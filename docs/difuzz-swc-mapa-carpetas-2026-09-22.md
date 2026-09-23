# difuzz (SWC) — Mapa de carpetas · 22-sep-2026

**Qué es este doc:** qué es cada carpeta del trabajo difuzz/SWC, de qué depende, y si se
puede tocar o borrar. Todo es específico de difuzz + SWC.

**Cómo leerlo:** lo marcado **VERIFICADO** salió de salida de comandos en las sesiones del
21 y 22-sep. Lo marcado **SUPUESTO** no se comprobó: el script del §5 lo confirma o lo
corrige. Antes de actuar sobre una carpeta, correr el script.

---

## 1. Vista rápida

```
~/difuzz                         ← LA HERRAMIENTA (git, fuente de verdad)
~/difuzz-main                    ← copia vieja de difuzz, apunta a ~/swc-main (SIN git)
~/difuzz-main2                   ← PROPUESTA, no existe todavía
~/difuzz-bisect                  ← descartable

~/Desktop/OpenSource/swc         ← CLON PRINCIPAL de SWC (tiene el .git real)
~/swc-main                       ← upstream/main de SWC (lo usa ~/difuzz-main)
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
| `~/difuzz/difuzz-out` | salida por defecto: `div/`, `known/`, `panic/` | ignorado o no: SUPUESTO | — | **no a mano**: el reporter la limpia al arrancar. Contiene la **base del 22-sep** (9 divergencias) |
| `~/difuzz/docs` | que-sigue, lecciones, estado | versionado | — | solo con commit |
| `~/difuzz-main` | copia scratch **vieja**: sin `known.rs`, 4 archivos distintos, sin `--jobs`, `<corpus_dir>` posicional · VERIFICADO | **no es repo git** · VERIFICADO | `[patch.crates-io]` → 6 crates en `/home/baltasarblanco/swc-main/crates/…` · VERIFICADO | **no**: es la única forma de reproducir la base del 21-sep (19 divergencias) y no hay git para volver atrás |
| `~/difuzz-main2` | **propuesta**: código de `~/difuzz` + el `[patch.crates-io]` de `~/difuzz-main` | — | `~/swc-main` | se crea cuando se decida |
| `~/difuzz-bisect` | bisect de una sesión anterior | SUPUESTO | SUPUESTO | descartable según el prompt de sesión; confirmar con el script antes de borrar |

**Las dos bases de comparación, que no se mezclan:**

| base | binario | SWC | divergencias | dónde vive |
|---|---|---|---|---|
| 21-sep | `~/difuzz-main` (viejo) | `~/swc-main` @ `4b42cd59d7` | 19 | se reproduce corriendo `~/difuzz-main` |
| 22-sep | `~/difuzz` (nuevo) | crates.io 61.0.8 | 9 | `~/difuzz/difuzz-out/div` |

Advertencia sobre `~/difuzz-main2`: comparar su corrida contra la base del 22-sep cruza dos
SWC distintos (crates.io 61.0.8 contra `main`). Todo cambio en `div/` que no sea `11084` se
explica con los commits entre las dos versiones antes de sacar conclusiones.

---

## 3. Carpetas de SWC

| carpeta | qué es | rama / commit | ¿se toca? |
|---|---|---|---|
| `~/Desktop/OpenSource/swc` | **clon principal**. Tiene el `.git` real; los worktrees guardan su metadata en `.git/worktrees/`. Remotos: `origin` = fork `baltasarblanco/swc`, `upstream` = `swc-project/swc` · VERIFICADO (push de #12401 fue a `baltasarblanco/swc.git`) | `fix/minifier-for-if-else-break` · VERIFICADO | **no** cambiar de rama ni commitear. `git fetch` sí |
| `~/swc-main` | `upstream/main` de SWC; lo usa `~/difuzz-main` por `[patch]` | `4b42cd59d7` · SUPUESTO (último valor conocido; está atrasado respecto de `42e4f7e98b`) | sí, para actualizarlo a `upstream/main`. Si es worktree, con `git -C ~/swc-main switch --detach upstream/main` |
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
echo "=== difuzz-out (base 22-sep)"
ls ~/difuzz/difuzz-out/div 2>/dev/null | wc -l
echo "=== @swc/core del probe"
node -e "console.log(require('$HOME/difuzz/probe/node_modules/@swc/core/package.json').version)" 2>/dev/null || echo "no está en ~/difuzz/probe"
```

Qué mirar en la salida:
- `worktree list` dice si `~/swc-main` es worktree del clon principal o un clon aparte.
- `git-dir` de un worktree apunta a `.../swc/.git/worktrees/<nombre>`.
- `du` muestra el peso real; los `target/` de Rust suelen ser lo que más ocupa.
- `difuzz-out/div` tiene que tener 9 entradas.

---

## 6. Qué se puede borrar, y cuándo

| qué | cuándo |
|---|---|
| `~/swc-12400` | después del merge o cierre de #12401, con `git worktree remove` |
| `~/difuzz-bisect` | cuando el script confirme que no tiene cambios sin commitear que importen |
| `~/difuzz-main` | **nunca mientras la base del 21-sep sea útil**. Si se quiere liberar espacio: `cargo clean` adentro (borra `target/`, conserva el código) |
| `target/` de cualquier copia de difuzz | cuando haga falta espacio; se regenera con `cargo build` |
| `/tmp/...` | solo; al reiniciar |
| `~/Downloads/...` de §4 | ya |

---

*Complementa a `difuzz-swc-que-sigue-2026-09-22.md` (versión de cierre) y
`difuzz-swc-estado-2026-09-21.md`. Actualizar cuando se cree `~/difuzz-main2`, se borre
`~/swc-12400` o se mueva `~/swc-main`.*
