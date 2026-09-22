# difuzz (SWC) — Qué sigue · 22-sep-2026

**Qué es este doc:** la cola de trabajo al cierre de la sesión del 22-sep. Complementa `difuzz-swc-estado-2026-09-21.md`, que no reemplaza. Todo es específico de SWC (`swc_ecma_minifier` 61.0.8, `main` @ `4b42cd59d7`, `@swc/core` 1.16.2).

---

## Dónde quedó la herramienta (commit `6b81374` + el de `Config::NAME`)

- **Pipeline igual al producto**: `paren_remover` antes del resolver, y la compresión construida con `TerserCompressorOptions::default().into_config()` (misma ruta que `swc::Compiler::minify`). Los cuatro `delete_*` eran un artefacto de no correr `paren_remover`: `handle_known_delete` no matchea `Paren(Cond)`.
- **Paralelo**: `--jobs N` (default: núcleos). Corpus completo: **2 min 07 → 15 s**.
- **Robusto**: `catch_unwind` por caso; un panic del fuzzer pierde el caso, no el worker.
- **Reporter** limpia `div/`, `known/`, `panic/` al arrancar.
- Corrida del 22-sep: `total=3102 iguales=1894 DIVERGENCIAS=9 conocidas=5 PANICS=0 observados=524`.

## Las 9 divergencias vivas

| caso | estado | producto (defaults, en función) |
|---|---|---|
| `fixture/issues/11084` | **#12397** reportado, fix **#12398** | `SyntaxError` |
| `drop_unused/unused_keep_harmony_destructuring` | **#12400** reportado, fix pendiente | `ReferenceError` |
| `fixture/issues/8953` | **listo para reportar** | `SyntaxError` ×6 combinaciones |
| `functions/issue_2101` | **candidato nuevo** (destapado por `paren_remover`) | `true → false`: IIFE inlineado pierde el `this` global |
| `collapse_vars/duplicate_argname` | candidato nuevo, uso común flojo | `PASS → FAIL`: `function (a, a)`, solo sloppy |
| `blocks/issue_1672_for` | real, Annex B, zona de Austaras | `SyntaxError` |
| `reduce_vars/lvalues_def_2` | real dentro de funciones | `2 NaN → 2 2` |
| `fixture/issues/do-while-false-terminal-jump/condition-exceptions` | real, exótico | `ReferenceError → unexpected` |
| `issue_973/this_binding_sequences` | baja: semántica de `eval`, fuera de policy | — |

Todas menos `this_binding_sequences` pasaron el probe contra `@swc/core` con defaults. Las nueve están en `passing.txt`: pasan con su config y rompen con otra.

## Qué sigue

Dos candidatos nuevos con evidencia de producto, y uno ya listo. Para no romper la regla de ritmo dos veces en un día: **el próximo reporte espera a la reacción de #12397 o #12400.**

### Pre-flight de `issue_2101` — hecho el 22-sep

- `gh search issues "IIFE this inlining"` y `"this global function inline"`: **sin resultados**.
- **PR #12267 de labor0** (*preserve new.target IIFE boundaries*, OPEN) toca `compress/optimize/iife.rs`, la misma pasada que inlinea la IIFE de `issue_2101`. Sus fixtures (`issues/12220/*`) cubren `new.target`, no `this`.

**Decisión pendiente:** `NO SE` si el fix de #12267 arregla el `this` de paso. Antes de reportar `issue_2101`:
1. Esperar el merge de #12267 **o** probar el repro contra su rama (`gh pr checkout 12267` en un worktree, `[patch]` como con `~/swc-main`).
2. Si sigue rompiendo, reportarlo citando #12267 como vecino y `iife.rs` como pasada.

Repro para el issue (ya medido en `@swc/core` 1.16.2 y nightly 20260913, defaults, con y sin mangle):
```js
a = {};
console.log((function () { return (function () { return this.a; })(); })() === (function () { return a; })());
// original: true · minificado: false
```

### Orden sugerido

1. **Fix de #12400** en chat nuevo (handoff en `difuzz-swc-estado-2026-09-21.md` §8.7).
2. Seguir **#12398**: review, CI, `./scripts/exec.sh` si no se corrió.
3. Al primer comentario de maintainer en #12397 o #12400: reportar **8953** (pre-flight primero: qué PR cerró el issue original, y si su fixture se verificó solo con su config, como el 11084).
4. **`issue_2101`**, según el punto de arriba.
5. `1672_for`: revisar ramas del fork de Austaras antes de invertir (`git ls-remote --heads`).

### Pendientes de difuzz (no bloquean)

- Matriz de configs (F2 del plan): ahora es viable, cada config cuesta ~15 s.
- Lint estático sobre el minificado para el punto ciego del observador (`[…][0] = x`).
- Nicho *fixture config ∪ pasadas apagadas*: leer cómo `tests/compress.rs` traduce `config.json`.
- Auditoría de snapshots (`input.js` vs `output.js`, sin compilar SWC): 129 divergencias crudas sin triage.
- `README.md` y hacer público el repo (F5).

## Errores propios del 22-sep

1. Predije 7 divergencias y salieron 9: no consideré que `paren_remover` podía **destapar** bugs además de tapar artefactos. Fue mejor que la predicción, pero la predicción estaba mal fundada.
2. El `tail -1` de un comando anterior cortaba la línea `total=`; ahora es lo único que va a stdout.
3. `Config.name` quedó sin leer al reordenar la construcción; un warning que el stub no mostró porque el stub lo leía distinto.
