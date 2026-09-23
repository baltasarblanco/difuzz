# difuzz (SWC) — Las 10 mejoras: diseño, implementación, medición · 23-sep-2026

**Qué es este doc:** el registro completo de las 10 mejoras propuestas a difuzz el 22/23-sep:
qué decía cada propuesta, qué se implementó realmente (y en qué se apartó de la propuesta),
cómo se midió, qué salió, qué errores aparecieron en el camino y qué queda pendiente. Todo es
específico de difuzz + SWC (`main` @ `42e4f7e98b`, `@swc/core` 1.16.2).

**Cómo leerlo:** **VERIFICADO** = salió de salida de comandos o de tests. **SUPUESTO** = no
comprobado. **PENDIENTE** = medición o paso que no se corrió todavía. **NO SE** = sin base.

**Docs hermanos:** cola de trabajo en `difuzz-swc-que-sigue-2026-09-23.md`; errores y
aprendizajes en `difuzz-swc-lecciones-2026-09-23.md`; carpetas en
`difuzz-swc-mapa-carpetas-2026-09-22.md` (§7, actualización del 23).

---

## 0. Resumen

- **Eje A (confianza): mejoras 1–5 implementadas, más tres commits de corrección (c6, c7, c8).**
  difuzz pasó de comparar **1.911** casos a **2.660** (+39 %); los descartes bajaron de
  **1.208** a **459**. Las divergencias quedaron separadas por gravedad
  (`ThrowNew`/`ThrowLost`/`ThrowType`/`Stdout`). El punto ciego del 9739 quedó cubierto por
  el lint, **validado** en `4b42cd59d7`.
- **Eje B (volumen): mejora 6 implementada** (matriz 2×2, más dos correcciones: 6b y 6c).
  `core_defaults` resultó **equivalente a `@swc/core`** (26 de 29 divergencias reproducen en el
  probe; las 3 restantes explicadas). **Las 12 divergencias de `carril_a` aparecen también en
  `core_defaults+en_funcion`: todas son uso común por construcción.**
- **Mejoras 7–10: pendientes.**
- Serie de commits (hashes del repo local, sin push al cierre de este doc): ver §12.

---

## 1. Punto de partida (base `out-main`)

Binario `~/difuzz-main2` (difuzz @ `9ac4db5` + `[patch]` a `~/swc-main` @ `42e4f7e98b`),
corpus fijo `~/difuzz-main2/corpus-4b42` (3119 `input.js`). **VERIFICADO:**

```
total=3119 iguales=1898 DIVERGENCIAS=8 conocidas=5 PANICS=0 observados=524
descartados: original=736 mudos_sin_bindings=105 parse=367
```

Comparados = iguales + divergencias + conocidas = **1.911**. Descartes = 736 + 105 + 367 =
**1.208** (39 % del corpus sin veredicto).

---

## 2. Eje A — mejora 1: desglose de descartes

**Propuesta.** `original_discarded` se incrementaba en tres lugares con causas distintas
(`main.rs:333` A lanzó/colgó/murió; `main.rs:325` instrumentado sin salida; `main.rs:301`
mudo con `--no-observe`), y `parse_error` juntaba ESM, sintaxis rechazada y errores
recuperados. Contadores separados y un segundo intento de parse como módulo, solo para contar.

**Implementado (c1, `efcdbf2`).**
- `Stats`: `a_exit`, `a_timeout`, `a_signal`, `observed_not_ok`, `silent_no_observe`,
  `parse_esm`, `parse_other`. `original=` en la línea de stats sigue siendo la suma, con el
  detalle entre paréntesis (compatibilidad con greps viejos).
- `minify.rs`: `Outcome::ParseError { esm: bool }`; `parses_as_module` reintenta con
  `parse_file_as_module` (firma verificada contra `swc_ecma_parser` 45.1.4).

**Medido (VERIFICADO).** Mismos números que `out-main`; `original=736 (exit=730 timeout=6)`,
`parse=367 (esm=350 otro=17)`. **Hallazgo:** los 736 descartes eran casi todos originales que
**lanzaban** (730), no programas rotos: eso dimensionó la mejora 2.

---

## 3. Eje A — mejora 2: oráculo de excepciones

**Propuesta.** Dejar de descartar originales que lanzan (`main.rs:332`); comparar el tipo del
error, no el mensaje (trae nombres mangleados); TDZ en A → conocida.

**Implementado (c2, `339e820`).**
- `preload.js`: `uncaughtException` y `unhandledRejection` escriben `!dz throw <Tipo>` con el
  `write` capturado y hacen `process.exit(70)`. **`process.exit` y no `exitCode`:** después
  de un `uncaughtException`, node seguiría corriendo timers pendientes, cosa que un crash real
  no hace (VERIFICADO en node: el timer no corre).
- Tipo = primer built-in de `[AggregateError, EvalError, RangeError, ReferenceError,
  SyntaxError, TypeError, URIError, Error]` que matchee con `instanceof`; clases propias que
  extienden `Error` se ven como su built-in (su nombre también se manglea); primitivos como
  `valor <ser>`; resto `objeto`.
- Prefijo **`!dz `** para toda línea que escribe difuzz y no el programa (`runner::DZ`).
- `runner.rs`: `THROW_EXIT = 70`; `Exec::completed()` = exit 0 o 70; `ok_with_output` = terminó
  y (lanzó o imprimió algo propio); `ok_silent` = terminó, no lanzó, sin salida propia.
- `oracle.rs`: acepta `Exited(THROW_EXIT)` como resultado comparable; `Kind::Throw` (partido en
  tres en c8, ver §7).
- `main.rs`: contador `lanzaron=` (comparados cuyo original lanzó).
- TDZ: el flag `tdz` lo pone el handler (mensaje `before initialization`, VERIFICADO en node);
  la clasificación entra con el sensor (c3).

**Medido (VERIFICADO, corrida válida de c2).** `iguales=2615 DIVERGENCIAS=18 conocidas=8
lanzaron=730`, `original=6` (solo los 6 timeouts). Cierre: +717 iguales, +10 divergencias,
+3 conocidas = 730.

---

## 4. Eje A — mejora 3: sensor de asunciones

**Propuesta.** Reemplazar la tabla de rutas (`known.rs`) por detección de lo que el programa
usa: runtime (toString, `.stack`, Proxy, built-ins pisados) + AST (`arguments.callee`,
redefinición de `undefined`/`NaN`/`Infinity`). Persistir el síntoma de cada conocida para que
un cambio de síntoma la devuelva a `div/`.

**Implementado (c3, `4295728`).**
- **Runtime (`preload.js`):** `Function.prototype.toString` envuelto (flag `fn_tostring`);
  `Error.prepareStackTrace` (V8 lo llama al leer `.stack`; flag `stack`); `Proxy` envuelto con
  un Proxy (construct y `revocable`; flag `proxy`); foto de descriptores de built-ins al final
  del preload contra la foto al salir (flag `builtins`; `NaN !== NaN` contemplado); `tdz` desde
  el handler de excepciones. Los flags salen en stderr como `__DIFUZZ_FLAGS__ a,b`
  (`runner::FLAGS_TAG`), que `runner::split_flags` saca del stderr.
- VERIFICADO en node que **node mismo no dispara** `fn_tostring` al imprimir funciones
  (usa primordials) ni `stack` en un throw no atrapado en runtime; ni los globals perezosos
  (`fetch`, `crypto`, `structuredClone`) cuentan como creados.
- **AST (`checks.rs`, nuevo, `swc_ecma_visit` 29.0.0):** `callee` (`arguments.callee`, cualquier
  `.caller`), `redefine_global` (bindings o `function` llamados `undefined`/`NaN`/`Infinity`).
- **`known.rs`:** orden sensor → ruta → backlog; el motivo dice cuál clasificó
  (`sensor, asuncion: …` / `ruta, …` / `backlog de SWC …`). `known-sintomas.tsv` en `<out>`:
  `caso<TAB>hash` (FNV-1a de `kind` + stdouts normalizados), bajo un encabezado con el id del
  oráculo (FNV-1a de `PRELOAD` + celda). Si una conocida diverge con otro hash vuelve a
  `div/` (`conocidas_sintoma_nuevo=`) y el hash viejo **no se pisa** hasta que alguien borre
  su línea.
- **Dependencia nueva:** `swc_ecma_visit = "29.0.0"`. En `~/difuzz-main2` requiere su línea en
  `[patch.crates-io]` (lo hace `scripts/medir-serie.sh`).

**Error de diseño y su corrección (c6, `29ff4a6`).** c3 clasificaba con los flags de **A ∪ B**.
Node lee `.stack` al decorar un SyntaxError de carga (VERIFICADO), así que todo minificado con
SyntaxError salía con `stack` y pasaba a conocida: **escondió el 8953 (#12404) y el
`1672_for`**. Una asunción habla del **programa de entrada**: desde c6 clasifican solo los flags
de A + los estáticos del original; los de B se muestran aparte (`flags_a` / `flags_b` en
`meta.txt`). Un flag solo en B es hallazgo.

**Refinamiento (c7, `ab3202b`).** La asunción de AGENTS.md dice *"no depende del contenido"*
de `toString()`/`.stack`, no *"no los llama"*: `f++` sobre una función llama a `toString` y da
`NaN` igual. Ante una divergencia con `fn_tostring` o `stack`, A se corre de nuevo con el
contenido alterado (`DIFUZZ_SALT=1`: toString + `/*dz*/`, stack + `at dz`); si la salida no
cambia, el flag se descarta y queda `contenido_irrelevante`. La variable se borra de
`process.env` para que el programa no la vea (VERIFICADO). Devolvió a `div/`
`duplicate_argname` y `accept_duplicated_parameters…`.

**Flag nuevo (c7): `coercion`.** AGENTS.md: *"Calls to primitive coercion helpers such as
`toString` and `valueOf` are side-effect free"* (VERIFICADO en `~/swc-main/.../AGENTS.md`
L22-24). Estático: clave `valueOf`/`toString`/`[Symbol.toPrimitive]` en objeto literal o clase,
o asignada a un miembro. Cubre `comparing/keep_comparisons` ×2. Costo aceptado: puede tapar una
divergencia no relacionada en un programa que define el helper; queda visible en `flags_a`.

**Tabla de rutas (c8).** La fila de NaN se borró (la cubre `redefine_global`). **Queda la de
`issue_2719`:** el programa arma `g["call" + "er"]` en runtime (invisible al AST) y V8 le da a
cada función sloppy su propio `caller` como propiedad no configurable (VERIFICADO: no hay
dónde engancharse).

**Medido (VERIFICADO).** c3: `DIVERGENCIAS=10 conocidas=16` (con el error de A ∪ B).
c6: 14 conocidas = 4 backlog, 1 ruta (2719), 2 stack, 1 toString, 2 tdz, 4 redefine_global.
c7 y c8: 14 conocidas = 4 backlog, 1 ruta, 1 stack, 2 tdz, 4 redefine_global, 2 coercion
(salen `duplicate_argname` y `accept_duplicated_parameters…` por la corrida salada; entran
los dos `keep_comparisons`).

---

## 5. Eje A — mejora 4: canales que el oráculo no miraba

**Propuesta.** stderr, globals creados, serializador (prototipo, claves ocultas), recorte
silencioso del epílogo.

**Implementado (c4, `be6120d`).**
- `console.error`/`warn`/`trace` → stdout con prefijo `[error] ` etc. (`util.format`; trace
  sin stack). `info`/`debug` ya van a stdout en node.
- Al salir: `Reflect.ownKeys(globalThis)` contra la foto del preload; cada global nuevo sale
  como `!dz global <nombre> = <valor>` (accessors como `[accessor]`, sin invocar). No toca el
  fuente: **sin efecto observador**.
- Serializador: prefijo de prototipo (`''` plano, `null`, `inst`); `Reflect.ownKeys` con `~`
  para no enumerables y `[Symbol(x)]` para symbols (seguro mientras `mangle.props` siga
  apagado).
- Epílogo: si hay más de `MAX_OBSERVED` (30) bindings, agrega `["!dz truncados", N]`.
- `main.rs`: un mudo sin bindings pero con globals cuenta como observado
  (`observados_por_globals=`, subconjunto de `iguales`). Las líneas `!dz ` no cuentan como
  salida propia, así el mudo sigue yendo por el observador.

**Falso positivo introducido y su corrección (c7).** La redirección de stderr capturó los
avisos de node (`(node:PID) V8: … Invalid asm.js`), que traen el **PID**:
`skipped-scopes` y `asm_toplevel` divergían solo por eso. Corrección: node con
`--no-warnings` (VERIFICADO: el aviso desaparece).

**Medido (VERIFICADO).** c4: `mudos_sin_bindings` 105 → **83**, `observados_por_globals=22`,
3 divergencias nuevas: las 2 del PID (falsos positivos, corregidos) y **`issue_2425_3`**
(real: el minificado crea un global `b` que el original no crea; solo visible por este canal).

---

## 6. Eje A — mejora 5: lint de escrituras sobre literales frescos

**Propuesta.** El observador agrega un uso y apaga el disparador del 9739 (punto ciego
estructural). Contar en el AST del original y del minificado las escrituras cuyo destino es un
literal fresco (`[…][0] = x`, `({}).k = x`, `[[a][0]] = …`); si el minificado tiene más, `lint/`.

**Implementado (c5, `2d6e2d6`).** `checks::dead_writes`: `AssignExpr` con destino
`Simple(Member)` sobre literal, `UpdateExpr`, y `Pat::Expr` (destinos de destructuring y
for-in/of; VERIFICADO en el parser que un miembro en patrón es `Pat::Expr`). Objeto "fresco"
solo si escribirle no corre código: sin getters/setters, sin `__proto__`, sin claves string.
Corre sobre la pasada **cruda**. Salida en `lint/<caso>/` y contador `LINT=`. Es
**sospechoso, no veredicto**.

**Medido y validado (VERIFICADO).**
- `42e4`: `LINT=6`, **los 6 equivalentes** (el objeto escrito no tiene usos posteriores;
  p. ej. `var foo={}; (foo.bar={}).test=123` → `(({}).bar={}).test=123`). Precisión 0.
- `4b42` (bug vivo, control: `11084` en `div/`): `LINT=7` = **`fixture__issues__9739`** + los
  mismos 6. **El lint cierra el punto ciego que lo motivó.** Precisión 1/7 en un commit con el
  bug vivo.
- **PENDIENTE:** persistir el triage del lint (los 6 equivalentes) por hash del minificado,
  para que solo aparezca lo nuevo.

---

## 7. Eje A — correcciones que no estaban en la propuesta (c8, `24a529f`)

- **Original que node no acepta.** El parser de SWC es más permisivo que node
  (`function b({c: test, c: test})` es SyntaxError en node). Si A lanza `SyntaxError` sin haber
  impreso nada, se corre `node --check`; si falla, descarte `invalido=`. Medido: **3**
  (uno era `accept_duplicated_parameters…`, que estaba en `div/`; dos contaban como iguales).
- **`Kind::Throw` en tres:** `ThrowNew` (el minificado lanza y el original no: el minifier
  introdujo el error, la clase del 8953), `ThrowLost` (el original lanza y el minificado no:
  suele ser código muerto que lanzaba; baja prioridad), `ThrowType` (tipos distintos).

**Estado final del eje A (c8, VERIFICADO):**

```
total=3119 iguales=2634 DIVERGENCIAS=12 conocidas=14 LINT=6 observados=524
observados_por_globals=22 lanzaron=727
descartados: original=9 (timeout=6 invalido=3) mudos_sin_bindings=83 parse=367 (esm=350 otro=17)
```

Comparados **2.660** (+39 %), descartes **459**. Divergencias: 6 `Stdout`, 3 `ThrowNew`,
3 `ThrowLost`.

---

## 8. Eje B — mejora 6: matriz de configuraciones

**Propuesta.** `{carril_a, core_defaults} × {tal_cual, en_funcion}`; `core_defaults` =
`minifySync(src, {compress: {}, mangle: {}})`; `en_funcion` = wrapper de `probe.cjs`; caché de A;
saltear B si el minificado es idéntico; dedup entre configs.

**Implementado (m6 `bdb0376`, m6b `2674434`, m6c `cd623e6`).**
- CLI: `--config carril_a|core_defaults`, `--forma tal_cual|en_funcion`. Sin flags, idéntico a
  antes. La celda (`<config>+<forma>`) nombra la salida y entra en el id del oráculo.
- `Config::core_defaults()`: `TerserCompressorOptions::default().into_config` sin tocar +
  `MangleOptions::default()` (sin `top_level`, sin `keep_*`).
- `Forma::EnFuncion`: `(function () {\n<src>\n})();\n`, carácter por carácter igual a
  `probe.cjs`. Se aplica **después** del epílogo observador (el epílogo queda dentro de la
  función y ve los bindings).
- `scripts/matriz.sh`: corre las 4 celdas en `<out>/<celda>` y arma la tabla cruzada (una fila
  por caso, `X` por celda, columnas `core_defaults` primero).
- **Desvíos de la propuesta:** sin caché de A (cada celda es un proceso; reescribir
  `process_case` sin compilador no se justificaba); sin salto de B idéntico; el dedup lo hace
  el script, no el reporter. Los tres quedan **PENDIENTES** (§10).

**Validación (VERIFICADO).**
- Regresión: la celda `carril_a+tal_cual` dio exactamente c8 (stats y `div/`).
- Determinismo: dos corridas completas de la matriz, salida idéntica.
- **Equivalencia con `@swc/core`:** probe sobre cada divergencia de `core_defaults`: 26/29 con
  `✗` en la misma forma y `defaults_mangle` (11/11 en `tal_cual`, 15/18 en `en_funcion`;
  recontado sobre la salida). Las 3 sin `✗`: `issue_2425_3` (el bug es un global;
  el probe solo mira stdout), `unused_keep_harmony_destructuring` (se ve por el epílogo; el
  probe compara crudo), `11684/class-decl` (en el probe el original lanza `G is not defined`;
  NO SE por qué difiere, y en difuzz es de nombres).

**Corrida m6 (VERIFICADO):**

| celda | iguales | div | conocidas | LINT |
|---|---|---|---|---|
| core_defaults+en_funcion | 2629 | 18 | 16 | 6 |
| core_defaults+tal_cual | 2630 | 11 | 19 | 3 |
| carril_a+en_funcion | 2641 | 12 | 10 | 6 |
| carril_a+tal_cual | 2634 | 12 | 14 | 6 |

Efecto lateral de `en_funcion`: un ESM envuelto no parsea ni como script ni como módulo
(`parse … (esm=1 otro=364)`); esperable mientras no haya carril B.

**Falsos positivos de nombres y su corrección.** 6 de los 7 casos exclusivos de
`core_defaults` eran nombres (`keep_*` apagado): `negate_iife_issue_1073` ×2 (`Foo { x: 7 }` →
`{ x: 7 }`), `12306` ×2 y `issue_2416` (leen `.name`), `11684/class-decl` (`A {}` → `n {}`).
- **m6b:** en `core_defaults`, ante una divergencia se reminifica el original impreso con
  `keep_*` en `true` (`Config::core_defaults_keep_names`); si la divergencia desaparece, flag
  `nombres` → conocida.
- **Error de m6b, corregido en m6c:** `keep_*` no solo preserva nombres, **también apaga
  optimizaciones** (inlining de funciones con nombre). Con eso **`12188` (real) pasó a conocida
  como `nombres`**. Desaparecer con `keep_*` es condición necesaria, no suficiente. m6c exige
  además `names_visible`: el original lee `.name`, o su salida imprime una instancia con nombre
  de clase (`Ident {`). Test con los casos reales.
- **Resultado de m6c (VERIFICADO, `out-matriz-c`):** `12188` volvió a la tabla, solo en `C.fn`;
  las conocidas por `nombres` bajaron de 16 a 15 (la que salió es `12188`). Las celdas
  `carril_a` no cambiaron.

**Corrida m6c, estado final de la matriz (VERIFICADO):**

| celda | iguales | div | conocidas | LINT |
|---|---|---|---|---|
| core_defaults+en_funcion | 2629 | 13 | 21 | 6 |
| core_defaults+tal_cual | 2630 | 7 | 23 | 3 |
| carril_a+en_funcion | 2641 | 12 | 10 | 6 |
| carril_a+tal_cual | 2634 | 12 | 14 | 6 |

Conocidas de las dos celdas `core_defaults` por motivo: 17 backlog, 15 nombres, 4
redefine_global, 2 ruta (2719), 2 stack, 2 tdz, 2 coercion.

**Corrida m6b (VERIFICADO, parcial):** `core_defaults+tal_cual`: `iguales=2630
DIVERGENCIAS=7 conocidas=23`; celdas `carril_a` sin cambios; 16 conocidas por `nombres` entre
las dos celdas `core_defaults` (incluye casos del backlog reclasificados, porque el sensor
tiene prioridad sobre el backlog). La línea de `core_defaults+en_funcion` quedó fuera del
`tail`: no registrada.

---

## 9. Triage de todas las divergencias (al cierre de m6b)

**Uso común por construcción** = `X` en una celda `core_defaults`. **Probe** = reproduce en
`@swc/core` 1.16.2.

| caso | kind | celdas | veredicto |
|---|---|---|---|
| `fixture/issues/8953` | ThrowNew | las 4 | **#12404** reportado (regresión de #10201) |
| `blocks/issue_1672_for` | ThrowNew | las 4 | real, Annex B (`SyntaxError` en el minificado), zona de Austaras |
| `drop_unused/unused_keep_harmony_destructuring` | ThrowNew | C.fn, A.fn, A.tc | **fix en #12401** (abierta). Se ve por el epílogo: el minificado deja `__difuzzObserve(["foo",foo])` sin la declaración. Con un uso real (`console.log(typeof foo, foo)`) no diverge en ninguna celda (VERIFICADO, `/tmp/ukhd`): el disparador de #12400 es que `foo` solo se referencie a sí misma |
| `functions/issue_2101` | Stdout | las 4 | candidato; depende de **#12267** (labor0, OPEN, `iife.rs`) |
| `issue_973/this_binding_sequences` | Stdout | las 4 | baja: semántica de `eval`, fuera de policy |
| `do-while-false-terminal-jump/condition-exceptions` | Stdout | las 4 | real, exótico |
| `reduce_vars/lvalues_def_2` | Stdout | C.fn, A.fn, A.tc | real dentro de funciones (`2 NaN` → `2 2`) |
| `collapse_vars/duplicate_argname` | Stdout | C.fn, A.fn, A.tc | **familia parámetros duplicados** (ver abajo) |
| `collapse_vars/issue_2425_3` | Stdout | C.fn, A.fn, A.tc | **familia parámetros duplicados**: filtra el global `b` |
| `fixture/issues/11645/spread-argument-preserved` | ThrowLost | C.fn, A.fn, A.tc | **spread** (ver abajo) |
| `collapse_vars/reassign_const_1` | ThrowLost | las 4 | baja: programa inválido (asignación a `const`) |
| `comparing/dont_change_in_or_instanceof_expressions` | ThrowLost | las 4 | baja: `1 in 1` lanza y se borra como código muerto |
| `fixture/issues/12188` | Stdout | C.fn | candidato ya conocido (estado 21-sep: fix #12253 mergeado y diverge igual). Hoy solo con defaults dentro de función; en 1.16.2 falla en otra línea (`member test 1` → `0`) |

**Familia parámetros duplicados** (`duplicate_argname`, `issue_2425_3`). En sloppy, con
parámetros duplicados gana el **último** argumento; cuando los argumentos tienen efectos, el
inliner toma el **primero** o lo asigna a una variable no declarada. Casos simples sin efectos:
bien. Reproduce con defaults dentro de función (VERIFICADO en el sandbox con 1.16.2). Uso común
muy flojo (solo sloppy; un linter lo rechaza) y toca `iife.rs` (misma zona que #12267).
Prioridad **baja**.

**Spread.** `function f(a){return a} function* gen(){console.log("iterado"); yield 1}
f(1, ...gen());` dentro de función, defaults: se pierde `"iterado"`. El minificado conserva la
llamada `gen()` pero **no la iteración** del spread. Mismo resultado con un iterador propio.
**De larga data:** VERIFICADO en `@swc/core` 1.3.100 (2023), 1.10.0, 1.15.8, 1.15.18, 1.15.21 y
1.16.2 (instalados en el sandbox, ejecutando la salida). No es regresión de #11645 (PR de
Austaras, *Remove useless arguments for non inlined callee*; su fixture no tiene
`config.json` y cubre solo el nivel superior). Sin reporte previo encontrado. Prioridad
**media-baja** (patrón raro en código real).

**Ya no son divergencias:** `accept_duplicated_parameters…` (original inválido para node),
`skipped-scopes` y `asm_toplevel` (PID del aviso de node), `keep_comparisons` ×2 (`coercion`,
asunción documentada), los 6 de nombres de `core_defaults`.

---

## 10. Pendientes de las mejoras

**De verificación:** hechos (m6c aplicado, 11 tests; `out-matriz-c` con `12188` de vuelta).
Queda solo el `git push` (decisión del usuario).


**De la mejora 6 (desvíos):**
3. Caché de A por hash del original (A no depende de la config): 6 ejecuciones por caso en vez
   de 8 en la matriz. Hoy la matriz tarda ~10 min (4 corridas).
4. Saltear B cuando el minificado es idéntico al original.
5. Dedup entre celdas dentro del reporter (hoy lo hace `matriz.sh`).
6. Clasificación de parse en `en_funcion` (ESM envuelto cuenta como `otro`): cosmético.

**Del eje A:**
7. Persistir el triage del lint por hash del minificado.
8. `issue_2719` sigue por ruta: solo se sacaría con un flag de acceso computado a `caller`.

**Mejoras 7–10 (sin empezar):**
9. **7 · invocar funciones observadas** (el cuerpo de funciones nunca llamadas hoy no se
   ejecuta; llamarlas es sólido porque ya escapan). Depende de 2 y 3 (hechas).
10. **8 · carril B, módulos** (350 ESM hoy descartados; observar el namespace de exports sin
    efecto observador; `--require` con `.mjs` ya VERIFICADO).
11. **9 · corpus de `tests/exec.rs` y `terser_exec.rs`** (programas que imprimen por diseño,
    con su `config.json`: insumo del nicho *config del fixture ∪ pasadas apagadas*).
    `benches/full/`: `// VERIFICAR` que exista.
12. **10 · mutaciones** (borrar una sentencia: solo pasada cruda + lint, nunca observador;
    perturbar literales).

---

## 11. Técnicas nuevas que quedaron disponibles

- **Medir commit por commit** contra una base con criterio escrito antes de correr
  (`scripts/medir-serie.sh`). Detectó 3 errores de esta serie.
- **Probe por release:** instalar varias versiones de `@swc/core` en directorios temporales
  (`npm i @swc/core@X`) y **ejecutar** la salida minificada de cada una. Fecha un bug sin
  compilar SWC (así se fechó el spread en ≤ 1.3.100). Chequear ejecutando, no con
  `includes(...)` sobre el código (ver lecciones §5).
- **Corrida "salada"** para distinguir llamar a una API de depender de su contenido.
- **Verificación contrafactual** (reminificar con otra config y ver si la divergencia
  desaparece), con la precondición de que la otra config no cambie más que lo que se quiere
  aislar (ver m6c).

---

## 12. Serie de commits en `~/difuzz` (sobre `9ac4db5`)

| id | hash | qué |
|---|---|---|
| c1 | `efcdbf2` | desglose de descartes |
| c2 | `339e820` | oráculo de excepciones |
| c3 | `4295728` | sensor de asunciones + `known-sintomas.tsv` |
| c4 | `be6120d` | canales (stderr, globals, serializador, recorte) |
| c5 | `2d6e2d6` | lint de escrituras sobre literales |
| c6 | `29ff4a6` | el sensor clasifica solo con flags del original |
| c7 | `ab3202b` | `--no-warnings`, flag `coercion`, corrida salada |
| c8 | `24a529f` | `ThrowNew`/`ThrowLost`/`ThrowType`, `invalido`, tabla sin NaN |
| m6 | `bdb0376` | matriz (`--config`, `--forma`) |
| m6b | `2674434` | `nombres` en `core_defaults` |
| m6c | `cd623e6` | `names_visible` (keep_* tapaba el 12188) |
| docs | este commit | docs + `scripts/` |

Tests: 9 (c1–c8) → 10 (m6) → 11 (m6c).
