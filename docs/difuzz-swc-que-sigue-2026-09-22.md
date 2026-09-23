# difuzz (SWC) — Qué sigue · 22-sep-2026 (cierre)

**Qué es este doc:** la cola de trabajo al cierre del 22-sep. **Reemplaza** a la versión de
la mañana del mismo nombre y complementa `difuzz-swc-estado-2026-09-21.md`. Todo es
específico de SWC: `main` @ `42e4f7e98b`, `@swc/core` 1.16.2.

---

## Qué pasó hoy

- **#12398 MERGEADA** por kdy1 (squash `42e4f7e98b`, ~19 h, sin pedidos de cambio).
  #12397 quedó `CLOSED COMPLETED`. Lecciones en `lecciones-pr-12398-swc.md`.
- **#12401 abierta**: fix de #12400 en `simplify/dce/mod.rs` (no en el minifier). Rebaseada
  sobre `42e4f7e98b`, CI corriendo al cierre, esperando review. Lecciones en
  `lecciones-pr-12401-swc.md`.
- **#12404 reportado**: `fixture/issues/8953` es una **regresión** de #8953, introducida por
  #10201 (kdy1, 2025-03-20). Sin compromiso de fix.
- **Noche: `11084` confirmado fuera de `div/` en `main`.** A/B con el mismo binario
  (`~/difuzz-main2`) y el mismo corpus (`corpus-4b42`, 3119 `input.js`): `4b42cd59d7` → 9
  divergencias, `42e4f7e98b` → 8. `diff` de `div/`: solo `< fixture__issues__11084`;
  `known/` sin cambios, 0 panics. Entre los dos commits solo hay #12398 y #12392 (Austaras,
  refactor de dead code), que no cambió nada observable en el corpus.
- **Estado al cierre de la noche:** #12401 sin review, CI sin rojos con 4 jobs en progreso
  (`Test - swc_cli - windows-latest`, `Test - swc - windows-latest`,
  `Test - swc - ubuntu-latest`, `Test node bindings - windows-latest`). #12404: 0
  reacciones, timeline vacío.

## Las 9 divergencias de la corrida del 22-sep, estado actual

| caso | estado |
|---|---|
| `fixture/issues/11084` | **arreglado** en `main` (#12398). **Confirmado** fuera de `div/` con A/B `4b42cd59d7` → `42e4f7e98b` (noche del 22-sep) |
| `drop_unused/unused_keep_harmony_destructuring` | fix en **#12401** (abierta); difuzz con el fix: sale de `div/` y nada más cambia |
| `fixture/issues/8953` | **#12404** reportado (regresión de #10201) |
| `functions/issue_2101` | candidato; depende de **#12267** (labor0, OPEN, toca `iife.rs`) |
| `collapse_vars/duplicate_argname` | candidato, uso común flojo (`function (a, a)`, solo sloppy) |
| `blocks/issue_1672_for` | real, Annex B, zona de Austaras |
| `reduce_vars/lvalues_def_2` | real dentro de funciones |
| `fixture/issues/do-while-false-terminal-jump/condition-exceptions` | real, exótico |
| `issue_973/this_binding_sequences` | baja: semántica de `eval`, fuera de policy |

## Datos de #12404 (8953) para un eventual fix

- Historial del `output.js`: `7344a638b5` (#9677) válido → `6841523977` (#10201) inválido →
  `1115d37049` (revert) válido → `7cd3dac138` (re-land) inválido, igual en `main`.
- El snapshot tiene **congelado el bug**: si se toma el fix, el `output.js` vuelve a algo
  equivalente a `console.log(1)`, y eso va justificado en el body (como `9739` en #12398).
- Solo `async function` diverge; `function`, `function*`, sloppy y módulo pliegan a `1`
  (probe de 5 variantes, `@swc/core` 1.16.2). Coincide con el reporte original.
- `NO SE` qué parte de #10201 arma el `if` sin bloque. Punto de partida:
  `git show 6841523977 --stat` y la conversión `switch` → `if` en el pure optimizer.

## Orden sugerido

1. **#12401**: mirar la CI al terminar
   (`gh pr checks 12401 -R swc-project/swc --json name,state,link`, filtrar
   `FAILURE`/`CANCELLED`; los que no son `SUCCESS`/`SKIPPED`/`NEUTRAL` siguen corriendo). Ante un rojo, checklist §10 de `lecciones-pr-12398-swc.md`.
2. **Regla de ritmo:** el próximo reporte espera una reacción de maintainer a **#12404 o
   #12401**.
3. **#12404**: si hay señal de que kdy1 no lo toma, localizar la pasada en #10201 y
   comentar *"I'll send a fix"* recién con el fix localizado.
4. **`issue_2101`**: esperar el merge de #12267 o probar el repro contra su rama.
5. **`1672_for`**: revisar ramas del fork de Austaras antes de invertir.

## Mantenimiento de entorno

- `~/swc-main` **actualizado** a `42e4f7e98b` (detached, limpio). Hecho la noche del 22-sep.
- `~/difuzz-main2` **creado**: código de `~/difuzz` + el `[patch.crates-io]` hacia
  `~/swc-main`. Tiene el corpus fijo `corpus-4b42` y las corridas `out-4b42` (9) y
  `out-main` (8). Es el binario para medir `main` de ahora en adelante.
- `~/difuzz-main` sigue atado a `~/swc-main` por `[patch]`: al mover `~/swc-main` dejó de
  reproducir la base del 21-sep. Para reproducirla hay que volver `~/swc-main` a
  `4b42cd59d7` antes de compilarlo.
- `~/swc-12400` (worktree de la rama de #12401): borrar con `git worktree remove` **después**
  del merge, no antes.
- Ubicaciones y bases de comparación: ver `difuzz-swc-mapa-carpetas-2026-09-22.md`.

## Pendientes de difuzz (no bloquean)

- Matriz de configs (F2 del plan): ~15 s por config.
- Lint estático sobre el minificado para el punto ciego del observador (`[…][0] = x`).
- Nicho *fixture config ∪ pasadas apagadas*: leer cómo `tests/compress.rs` traduce `config.json`.
- Auditoría de snapshots (`input.js` vs `output.js`): 129 divergencias crudas sin triage.
  **Subió de prioridad:** #12398 (`9739`) y #12404 (`8953`) fueron snapshots con el bug
  congelado. Ejecutar cada `output.js` contra su `input.js` es barato y ya dio dos casos.
- Agujero anotado en `lecciones-pr-12398-swc.md` §8: `[(o.n)] = …` y `[super.x] = …` no
  marcan mutación. NO VERIFICADO que produzca un miscompile.
- `README.md` y hacer público el repo (F5).

## Errores propios del 22-sep (tarde)

Registrados en `lecciones-pr-12401-swc.md` §7. Los dos que se repiten de sesiones
anteriores: comandos que requieren espera pegados en el mismo bloque, y conteos esperados
anunciados sin recontar.

## Errores propios del 22-sep (noche)

Registrados en `lecciones-pr-12401-swc.md` §7 (8 y 9): `gh pr checks` sin `-R` desde
`~/difuzz`, y una base de comparación tomada del mapa sin verificarla.
