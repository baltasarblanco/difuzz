# difuzz (SWC) — Qué sigue · 23-sep-2026

**Qué es este doc:** la cola de trabajo vigente. **Reemplaza** a
`difuzz-swc-que-sigue-2026-09-22.md` (que queda como historial). El detalle de lo hecho está
en `difuzz-swc-mejoras-2026-09-23.md`; los errores y aprendizajes, en
`difuzz-swc-lecciones-2026-09-23.md`. Todo es específico de SWC: `main` @ `42e4f7e98b`,
`@swc/core` 1.16.2.

---

## Estado de SWC (lectura del 23-sep, madrugada)

- **#12401** (fix de #12400, `simplify/dce/mod.rs`): abierta, `MERGEABLE`, **CI verde**
  (último chequeo: sin pendientes ni fallidos), **sin review**. Rama
  `fix/12400-dce-self-ref-usage` @ `364447291a`. Worktree `~/swc-12400`: no borrar hasta el
  merge.
- **#12404** (8953, regresión de #10201): 0 comentarios, 0 reacciones, timeline vacío.
- **#12267** (labor0, `iife.rs`): abierta, sin aprobaciones, asignada a kdy1. Bloquea
  `issue_2101`.
- **Regla de ritmo:** el próximo reporte espera una reacción de maintainer a **#12401 o
  #12404**. Nada de lo de abajo se reporta antes.

## Estado de difuzz

- 14 commits sobre `9ac4db5`: 11 de código (c1–c8, m6, m6b, m6c `cd623e6`), 2 de docs y 1 de
  `Cargo.lock` (`90e71c4` al cierre). Push indicado al cierre del 23-sep: **verificar con
  `git -C ~/difuzz status -sb`** (sin `[ahead N]`) al arrancar.
- Eje de confianza (mejoras 1–5) cerrado y validado; mejora 6 (matriz) implementada.
- Binario de medición: `~/difuzz-main2` (código de `~/difuzz` + `[patch]` a `~/swc-main`
  @ `42e4f7e98b`, incluye `swc_ecma_visit`). Corpus fijo `corpus-4b42`.

## Cola, en orden

### A. Cerrar la mejora 6

1. ~~Aplicar m6c~~ hecho: `cd623e6`, 11 tests.
2. ~~Matriz `out-matriz-c`~~ hecho: `12188` de vuelta (solo `C.fn`); stats de las 4 celdas en
   `difuzz-swc-mejoras-2026-09-23.md` §8.
3. `git push` de los 14 commits: verificar que se hizo (ver arriba).

### B. Candidatos (preparar, no reportar: regla de ritmo)

| prioridad | candidato | qué falta |
|---|---|---|
| 1 | `fixture/issues/12188` (solo `core_defaults+en_funcion`) | leer #12188 y #12253; reducir; ver si alguien ya lo tiene; confirmar en `42e4` con el caso reducido |
| 2 | `reduce_vars/lvalues_def_2` (`2 NaN` → `2 2`, dentro de funciones) | reducir y buscar reporte previo |
| 3 | `condition-exceptions` (las 4 celdas) | reducir; exótico |
| 4 | `blocks/issue_1672_for` (ThrowNew, Annex B) | revisar ramas del fork de Austaras antes de invertir |
| 5 | spread de iterable con efectos (fixture de #11645) | redactar repro válido (`f(1, ...gen())`); de larga data (≤ 1.3.100) |
| 6 | parámetros duplicados (`duplicate_argname`, `issue_2425_3`) | solo sloppy; zona de `iife.rs` (#12267) |
| — | `issue_2101` | esperar #12267 o probar contra su rama |
| — | `unused_keep_harmony_destructuring` | es **#12401**: nada que hacer |
| — | 8953 | es **#12404** |
| — | `this_binding_sequences`, `reassign_const_1`, `in`/`instanceof` | baja / fuera de policy |

Para cada candidato, antes de tocar nada: leer su `config.json`, reducir, buscar en issues y
PRs, y confirmar en `main` con `~/difuzz-main2` (un corpus de un solo caso en `/tmp/<caso>/`
alcanza: `./target/release/difuzz /tmp/<caso> --config core_defaults --forma en_funcion`).

### C. Mejoras de difuzz

1. Persistir el triage del lint (hash del minificado).
2. Mejora 6, resto: caché de A, saltear B idéntico, dedup en el reporter.
3. **Mejora 7** (invocar funciones observadas).
4. **Mejora 8** (carril B, módulos: 350 casos).
5. **Mejora 9** (corpus de `tests/exec.rs` / `terser_exec.rs`; nicho *config ∪ pasadas
   apagadas*).
6. **Mejora 10** (mutaciones).

### D. Pendientes heredados (no bloquean)

- Auditoría de snapshots (`output.js` contra `input.js`): ya dio dos casos (#12398/9739,
  #12404/8953). Hacerla con `~/difuzz-main2`.
- Agujero de `lecciones-pr-12398-swc.md` §8 (`[(o.n)] = …`, `[super.x] = …`). NO VERIFICADO
  que produzca miscompile.
- `README.md` y hacer público el repo (F5 del plan).
- Subir al project knowledge: `difuzz-swc-mejoras-2026-09-23.md`,
  `difuzz-swc-que-sigue-2026-09-23.md`, `difuzz-swc-lecciones-2026-09-23.md`, mapa actualizado.

## Mantenimiento de entorno

- `~/swc-main`: `42e4f7e98b` (volvió de `4b42cd59d7` tras validar el lint).
- `~/difuzz-main2`: ver mapa §7. `Cargo.toml.antes-serie` es el `Cargo.toml` previo a la
  serie; el actual tiene además `swc_ecma_visit` en `[patch]`.
- Scripts versionados en `~/difuzz/scripts/`: `matriz.sh` (reutilizable) y `medir-serie.sh`
  (específico de la serie de confianza: mide `HEAD~5..HEAD`; ajustar si se reusa).
