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

---

## 7 · Addendum — 21-sep, tarde

**Fix de #12397:** PR **#12398**, abierto el mismo día que el issue. Un commit, CodSpeed sin impacto de performance, sin review al momento de anotar. Pendiente: el cuerpo dice que el reporte original reproducía *"with default options"*; nunca probamos defaults puros (corrimos carril A). Cambiar a "with `hoist_props` enabled" si no se verificó.

**Lo que el PR le enseñó a difuzz:** el fixture `issues/9739` tenía un snapshot que **congelaba el mismo bug** (el swap `[arr[0], arr[1]] = …` se convertía en escrituras sobre arrays nuevos). difuzz lo tenía en el corpus y no lo vio: el programa no imprime y el guard de salida vacía lo descartaba. En el corpus hay 623 casos así.

**Pasos 1 y 1b de §2: implementados** (commit "filtros de falsos positivos + epilogo observador"):
- Filtros: path fijo por proceso, `preload.js` con `--require` y `TZ=UTC`, cuatro `keep_*` en `true`.
- Epílogo observador: para programas mudos, comparación cruda primero y después una pasada con un epílogo que imprime los bindings top-level. El epílogo se agrega **antes** de minificar; el serializador vive en el preload y no se minifica.

**Validado en el sandbox (sin SWC):**
- `runner.rs` compilado aparte: 10/10 pruebas (determinismo, filename, timeout, streams, 10 MB sin deadlock, observador, `TZ`, limpieza).
- Pipeline completo con un `minify` stub que devuelve el `output.js` del fixture: 9739 pasa de `descartado` a `DIVERGENCIA Stdout (observado)`. El repro de `meta.txt` funciona copiado tal cual.
- Auditoría de snapshots (3.035 fixtures, `input.js` contra su `output.js`): 41 divergencias observadas = **1 real (9739)** + 34 `ReferenceError` (artefacto del método: el snapshot no tenía epílogo) + 5 de `mangle.props` + 1 bajo `unsafe_*`. Ninguna atribuible al serializador.
- Bugs propios de F1 encontrados en el camino: reporte suelto en `div/` cuando el corpus es un solo caso; reportes `.js` sensibles a un `package.json` de tipo módulo.

**Técnica candidata nueva — auditoría de snapshots:** comparar `input.js` contra el `output.js` congelado, con el mismo runner y observador. **No requiere compilar SWC.** Encuentra snapshots que congelaron un bug (así apareció el 9739). Hay 129 divergencias crudas sin triage; la mayoría probablemente sean configs con `unsafe_*` o nombres, pero no está verificado.

**Criterio de éxito de 1b en tu máquina:** contra `main`, el 9739 diverge como observado; contra la rama del fix (`601e1db`), no.

---

## 8 · Addendum — 21-sep, noche: policy del minifier y dos reportes más

**Publicado:**
- **#12400** — declaración de función borrada mientras sigue referenciada, cuando su cuerpo la asigna a una local no usada. `ReferenceError` con **defaults** de `@swc/core` 1.16.2 dentro de cualquier función. Reclamado en el mismo issue ("I'll send a fix").
- Comentario en **#12397**: impacto con defaults de `@swc/core` dentro de funciones (`SyntaxError`); top-level necesita `toplevel`.
- Body del **#12398** corregido: la frase *"reproduced with default options"* era falsa para top-level. Verificado con `grep -c` sobre el body publicado.

### 8.1 · La policy de `crates/swc_ecma_minifier/AGENTS.md` (commit `ff6a3c3b19`, 11-sep)

Mi doc de la PR #12290 decía que había que leerla antes de elegir el próximo candidato. **No se leyó hasta después de publicar #12397.** Lo que dice y lo que cambia:

- **Exime** de lógica defensiva: globals o built-ins monkey-patcheados, reasignación rara de `arguments.length`, **bindings introducidos por `eval` directo**, y **resolución dinámica dentro de `with`**.
- *"A required fix must demonstrate an impact on common usage or identify an explicit support contract that the change violates."*
- Asunciones documentadas (= lista de exclusiones de difuzz): `toString`/`valueOf` sin efectos y built-ins no pisados; `undefined`/`NaN`/`Infinity` no redefinidos; sin `arguments.callee`/`.caller`; no depender de `Function.prototype.toString()` ni de `Error.prototype.stack`; acceso a propiedades de objetos planos sin efectos (Proxy/watch); `document.all` no es null; asignar propiedades a clases no lanza; acceder a identificadores top-level declarados no tiene efectos; **TDZ no se preserva**; aritmética sin efectos, incluidas excepciones de `bigint` + `number`.
- Tests: no agregar cobertura nueva en `tests/terser`; usar `tests/fixture/issues` o `tests/exec.rs`. Correr `./scripts/exec.sh` después de cualquier cambio.

**Consecuencias para difuzz:**
1. **El sesgo `eval` del plan (§3.2: swc#12029, swc#11913) apunta a una categoría cerrada por escrito.** Es la lección §1.23 de la PR #12290, repetida en otra categoría. Se abandona como objetivo del generador.
2. **Carril A se mantiene, pero su mejor argumento cae** (eval sloppy y `with`). Queda como default coherente de SWC, nada más.
3. **Criterio de selección nuevo:** *uso común bajo configs que los fixtures no prueban*, no "mis clases históricas".
4. **Todo issue tiene que mostrar uso común.** Técnica que funcionó: probe contra `@swc/core` con opciones por defecto (§8.3).

### 8.2 · Cruce con el backlog conocido de SWC

`tests/TODO.txt` (301) y `tests/postponed.txt` (130) listan fixtures de terser que SWC sabe que no cumple, y labor0 trabaja ese backlog.

| candidato | lista | veredicto |
|---|---|---|
| `global_fns`, `issue_1830_2`, `inverted_var` | TODO | conocidos: fuera |
| `unsafe_evaluate_object_2` | postponed | conocido |
| `1672_for`, `lvalues_def_2`, `2719/warn`, `delete_*` ×4, `this_binding_sequences`, `unused_keep_harmony_destructuring` | passing | **pasan con su config, rompen con la nuestra**: evidencia del nicho |

`this_binding_sequences` (eval + strict) baja de prioridad por la policy.

### 8.3 · Probe contra `@swc/core` — paso estándar antes de cualquier issue

`npm i @swc/core` + `minifySync(src, cfg)` + ejecución de original y minificado. Independiente de difuzz, es el producto real, y responde dos preguntas que difuzz no puede: **¿reproduce con defaults?** y **¿en qué forma de uso común?**

| caso | defaults, top-level | `toplevel: true` | defaults, dentro de función |
|---|---|---|---|
| #12400 callback / export | ✓ correcto | ✗ `ReferenceError` | ✗ `ReferenceError` |
| #12397 `[o.n] = [2]` / `({done: s.done} = …)` | ✓ correcto | ✗ `SyntaxError` | ✗ `SyntaxError` |

La columna del medio es la que habíamos medido con difuzz; la de la derecha es la que prueba uso común (wrappers de módulo, IIFE).

### 8.4 · El criterio de 1b falló, y aun así 1b encontró #12400

- El 9739 dio `iguales` en `main` y en el fix. **Efecto observador:** el bug necesitaba que `arr` no tuviera usos posteriores, y el epílogo agrega uno. Mismo swap + `console.log(arr)` (11084, caso 2) → correcto. El observador **no puede** ver bugs cuyo disparador es "binding sin usos". La auditoría de snapshots había validado el serializador, no el diseño.
- Contracara: #12400 salió **solo** gracias al epílogo (fixture `unused_keep_harmony_destructuring` + una referencia externa a `foo`). Un bug de "uso que el minifier no cuenta" solo se ve si alguien usa el binding.
- Corrida completa con los filtros: 34 → **19 divergencias**, **524 programas mudos comparados**.
- Candidato para el punto ciego, sin implementar: **lint estático** sobre el minificado. Un target de asignación que es miembro de un literal (`[…][0] = x`) es la firma del 9739.

### 8.5 · Pendientes de código (no bloquean nada)

- Warnings `field 0 is never read` en `Outcome::ParseError` / `CodegenError` (venían de F1).
- Normalizar `[Function: …]` / `[class …]` en el oráculo: nombres **inferidos** (`2508_5/6`, `unsafe_evaluate_object_2`) que `keep_fnames` no protege.
- Skip-list de asunciones de la policy (§8.1), empezando por `NaN_and_Infinity_…`.
- Etiquetar automáticamente las divergencias cuyo fixture está en `TODO.txt` / `postponed.txt`.
- Probe `@swc/core` como script del repo.

### 8.6 · Errores propios de esta ronda

1. **No leí `AGENTS.md` del minifier** antes de publicar, aunque mi propio doc lo mandaba. Es el más caro: cambia el objetivo del proyecto.
2. Afirmé que el epílogo habría detectado el 9739 sin probarlo con el pipeline real.
3. Reduje **reescribiendo** el caso en vez de **borrar una sentencia por vez**, y perdí la sentencia disparadora (k01 vs k02).
4. m01 (`foo === foo`) fue un negativo fabricado por mi variante: el minifier lo pliega (§1.13b).
5. El primer borrador de #12400 tenía una generalización no medida (el `ReferenceError` del repro mínimo). La cazó contrastar contra la salida cruda, como pide el método.

### 8.7 · Próxima sesión

1. **Fix de #12400, en chat nuevo** (handoff abajo).
2. Seguir #12398: review, CI, `./scripts/exec.sh` si no se corrió.
3. Los pendientes de §8.5.
4. `1672_for` (función como cuerpo de un `for`, `SyntaxError`): antes de invertir, revisar territorio. Switch/case y for-init son zona de Austaras (`swc-caza-issues` §3).

**Handoff del fix de #12400:**
- Repro: `(function () { function handler() { var self = handler; } setTimeout(handler, 0); })();` → `ReferenceError` con defaults.
- Condición (Rust API, `main`): declaración de función + local **no usada** inicializada con **su propio nombre**. No reproduce con la local usada, con `foo;` suelto, con alias a otra función, ni con **expresión** de función: ese último es el precedente in-tree.
- Hipótesis `INF`, no confirmada: el analizador registra la referencia interna como **uso recursivo** (`report_usage` → `used_recursively` → no suma), y al eliminar la local algo descuenta un uso de `foo` que nunca se sumó. Alternativa: `subtract_cycles` de `simplify::dce` (tu terreno de #11963/#12029). Primer paso: los logs de `report_change!` / *"Dropping X because it's not used"* para saber qué pasada lo borra.
- Territorio: el usage analyzer es zona de Austaras. Chequear ramas de su fork (§1.21) antes de escribir código.
- Test: exec test en `tests/exec.rs`, que falle sin el fix. Correr `./scripts/exec.sh`.
