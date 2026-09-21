# difuzz (fuzzer diferencial del minifier de SWC) — Estado al 21-sep-2026

**Qué es este doc:** punto de retome. Se lee junto con `Plan_difuzz_fuzzer_SWC_v1_1.pdf` y `difuzz-plan-v1-2-delta.md`.
**Todo lo de acá es específico de SWC** (`swc_ecma_minifier` 61.0.8, `main` @ `4b42cd59d7`).

---

## 1 · Dónde quedó

| | estado |
|---|---|
| F0 pre-flight | **Cerrada.** Compila con Rust 1.98 stable (piso real ≥ 1.85, por `regress`). |
| F1 esqueleto | **Camina.** Corpus completo (3.102 casos) en ~2 min. Commit `91c9c75`, repo privado `github.com/baltasarblanco/difuzz`. |
| Primer hallazgo | **swc#12397** — `hoist_props` inlinea propiedades escritas vía patrón de destructuring. Fix incompleto de #11084 (cerrado por PR #11221). |
| Métricas del plan | M3 ✓ (divergencia real reducida). M1 parcial: falsos positivos todavía no ~0. M4 pendiente (confirmación de maintainer). |
| Fix de #12397 | En curso, **en otro chat**. |

Rutas: herramienta `~/difuzz` · copia de scratch con `[patch]` a main `~/difuzz-main` · worktree de `upstream/main` en `~/swc-main` · clon de trabajo `~/Desktop/OpenSource/swc` (rama `fix/minifier-for-if-else-break`, no tocar).

---

## 2 · Al volver, en este orden

**0 · Registrar el fix de #12397.** Número de PR, recepción de kdy1, tiempo a merge. Si hubo lección, doc `lecciones-pr-<N>-swc.md` aparte. La reacción al primer reporte de difuzz fija el ritmo de los siguientes.

**1 · Tres filtros de falsos positivos** (17 de las 32 divergencias en `main` son nuestras):

| FP | casos | cambio |
|---|---|---|
| Path del `.cjs` distinto entre A y B | `drop_console_1/2`, `issue_640/drop_console_2` | `runner.rs`: un solo path por proceso (las corridas son secuenciales) |
| `Date.now()` | `pass-1/3` | preámbulo determinista: `Date.now`, `performance.now`, `Math.random` congelados |
| Nombres de funciones/clases | 13 casos (`hoist_class` ×3, `negate_iife_1073` ×2, `2508_5/6`, `7591`, `36127`, `1709`, `2416`, `unsafe_evaluate_object_2`, `var_assign_4`) + `12306` | `keep_fnames` y `keep_classnames` (compress) y `keep_fn_names` y `keep_class_names` (mangle) en `true`. Los cuatro existen, verificado; todos `false` por defecto |

Criterio de éxito: re-corrida contra `main` con ~14 divergencias, todas en la lista de §3.

**2 · Dedup de los 14 candidatos contra labor0** (ver §3). No invertir en ninguno sin cruzarlo con sus PRs e issues.

**3 · Rediseñar la config en torno al nicho** (ver §4). Es el cambio grande de F2 y reemplaza a "mutaciones sintácticas" como prioridad.

**4 · Plan v1.3.** Integrar labor0, el nicho, y las correcciones de §6.

---

## 3 · Los 14 candidatos vivos en `main`

| caso | síntoma | cruce con labor0 | prioridad |
|---|---|---|---|
| `fixture/issues/11084` | `SyntaxError` | — | **reportado: #12397** |
| `fixture/issues/12188` | `1 1` → `1 undefined` | es **su** issue; fix #12253 MERGED y diverge igual | delicado; mirar config del fixture primero |
| `terser/.../issue_973/this_binding_sequences` | `undefined` → `object`: `"use strict"` + `eval("this")` pierde el límite strict | vecino de #12327 (OPEN, direct eval) | **alta: tu territorio (eval + strictness)** |
| `terser/.../reduce_vars/lvalues_def_2` | `2 NaN` → `NaN NaN` | ninguno a la vista | alta |
| `terser/.../issue_2719/warn` | `3` → `5` | ninguno a la vista | alta |
| `terser/.../drop_unused/issue_1830_2` | `1` → `undefined` | ninguno a la vista | alta |
| `fixture/issues/8953` | `SyntaxError`: `async function` en posición de sentencia | ninguno a la vista; ¿Annex B? (`optimize/mod.rs:1400`) | media |
| `fixture/issues/do-while-false-terminal-jump/condition-exceptions` | `ReferenceError` → `unexpected` | ninguno a la vista | media |
| `terser/.../dead_code/global_fns` | pierde el `RangeError` de `Array(NaN)` | vecino de #12265 (OPEN, RegExp) | baja |
| `terser/.../reduce_vars/inverted_var` | `(function c(){ c = 6; return c })()` → `6` | casi seguro #12323 (OPEN) | baja: probable duplicado |
| `delete_conditional_1/2`, `delete_binary_1/2` | `true` → `false` | #12244, #12231 MERGED y divergen igual | baja: ¿sub-caso o fix incompleto? |

Para cada uno, **antes de tocar nada**: leer el `config.json` del fixture (si fija una opción que nosotros no, puede ser divergencia esperada, como pasó con `12306`), reducir, y buscar en issues y PRs.

---

## 4 · Lo que cambió de la estrategia

**labor0-bot** hace testing diferencial del minifier desde principios de septiembre de 2026: ~70 PRs (#12230–#12335) en ~3 semanas, la mayoría mergeados por kdy1. Usa el mismo harness que difuzz (patrón `tests/exec.rs`). Consecuencias:

- **Amplitud: perdida.** Cualquier divergencia genérica la encuentra primero el bot. La tesis de la v1.1 ("cada divergencia es un candidato a PR") ya no vale sin filtro.
- **Nicho con evidencia propia:** fixtures de regresión verificados solo con su propia config. #12397 salió exactamente de ahí (fixture de #11084 + `hoist_props`). Técnica correcta: **config del fixture ∪ pasadas que el fixture dejó apagadas**. Reemplazar la config entera produce falsos positivos (caso `12306`).
- **NO SE** cómo `tests/compress.rs` traduce el `config.json` (formato terser) a `CompressOptions`, ni qué defaults aplica a lo que el JSON no nombra. Es lo primero que hay que leer para implementar el nicho.
- Ritmo: de a un reporte por vez, midiendo la recepción de #12397 antes de abrir el segundo.

---

## 5 · Hechos verificados (no re-investigar)

- Set de dependencias: `swc_common 26.0.0 · swc_ecma_ast 29.0.2 · swc_ecma_codegen 32.0.3 · swc_ecma_minifier 61.0.8 · swc_ecma_parser 45.1.4 · swc_ecma_transforms_base 49.0.1`. Idénticas en `main` al 21-sep → `[patch.crates-io]` a un worktree funciona sin conflictos.
- Carril A (script + CommonJS): `parse_file_as_script`, `compress.module: false`, `compress.top_level: Some(TopLevelOptions { functions: true })`, runner con `.cjs`.
- `compress.module` solo se copia a `in_strict`, que gatea tres optimizaciones. El tipo de AST (`Module`/`Script`) no cambia lo que hacen el analizador ni el mangler.
- `top_level()` = `top_level.functions || module`. Sin eso, el DCE top-level no corre.
- `mangle.eval` es `ignore_eval`: **una asunción insegura**, no una capacidad. Va en `false`.
- `mangle.props` va apagado mientras el generador sea el corpus: el mangling de propiedades es observable por reflexión.
- `exec_node_js` de SWC no tiene timeout y mezcla stdout con stderr. Por eso difuzz usa runner propio.
- El `bug.rs` de 2022 tiene siete errores de compilación, seis causas; verificado con `rustc` contra `main`. PR posible, pendiente de decidir (Decisión #2 del delta).
- Corpus: 3.102 `input.js`; ~1.375 corren y dicen algo como `.cjs`. `fixture/` es mayormente ESM (se descarta en el parse del carril A).
- Tiempos: corpus completo ~2 min 07 s en 32 núcleos.

---

## 6 · Errores propios de esta sesión

1. **`mangle.eval`**: lo propuse como primera config de F1 porque la doc del campo parecía apuntar a tus bugs. En el código es `ignore_eval`, una asunción insegura. Lo detectó leer el uso, no la doc.
2. **"No hay opciones `unsafe`"** (plan v1.1): falso, hay trece. La fuente era documentación de terceros.
3. **"En un módulo el minifier puede borrar cualquier cosa top-level"**: cierto para JS, falso para cómo decide SWC. El tipo de AST no importa; importan las opciones.
4. **"El fix de #11084 no activaba `top_level` (lección §1.13)"**: la variante `r03`, dentro de una función, lo refutó. La condición era `hoist_props`. La narrativa salió antes que el experimento.
5. **"labor0 corre con defaults y no mira el top-level"**: #12332 (*preserve script global vars*) lo desmiente.
6. **`cd … || exit 1`** en un comando interactivo: habría cerrado la terminal. Y `worktree remove` sin `--force` sobre un worktree modificado, parado adentro del directorio que se borraba.

Patrón común de 1, 3, 4 y 5: afirmar un mecanismo antes de tener el experimento que lo distingue. El remedio que funcionó fue el mismo las cuatro veces: reducir con un control positivo (r05) y variar una sola cosa.
