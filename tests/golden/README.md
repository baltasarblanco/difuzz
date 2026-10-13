# Golden de detección de difuzz

`verificar.sh [opciones] [BIN] [OUT]` corre `corpus/` (19 casos) en 8 celdas (swc|oxc × carril a|b ×
core_defaults|carril_a, `--no-cache --jobs 4`) y compara la clase de cada caso con `esperado.tsv`. Verifica
también que cada `CORRIDA.txt` diga `completa`, la identidad de stats por celda y que `--invocar` y `--stubs`
(oxc, core_defaults, A y B) no pierdan ningún div/panic de la celda base. Sale con 0 si todo coincide.

- `BIN` por defecto: `${CARGO_TARGET_DIR:-<repo>/target}/release/difuzz`. Lo esperado asume el oráculo sin debug
  assertions (`cargo build --profile oraculo`; difuzz lo busca en `<BIN>-oraculo`, `<dir de BIN>/../oraculo/difuzz`
  o `DIFUZZ_ORACULO`). Sin oráculo, `oxc-o7` y `oxc-eval-indirecto` van a `panic/` y el script avisa.
- `OUT` por defecto: un temporal que se borra si todo pasa.
- Opciones: `--jobs N` (o `VERIFICAR_JOBS`), `--corpus`, `--esperado`, `--celdas "..."`, `--sin-modos`.
  `tests/aceptacion/rechequeos/verificar.sh` reutiliza este script con su corpus y sus 4 celdas.

Clases: `div:<kind>`, `panic`, `recursos`, `asserts`, `known`, `lint`, `descarte:<causa>`, `igual` (en ese orden
de precedencia). Una celda sin fila en `esperado.tsv` espera `igual`; una fila que no corresponde a un caso o a
una celda corrida cuenta como falla.

Por qué cada clase esperada:
- `oxc-o1`, `oxc-o3*`: el minificado de oxc es un SyntaxError en script y en módulo, así que ThrowNew en las 4 celdas de oxc.
- `oxc-o2-casos` (imprime `"ok"`): en sloppy, asignar al nombre de una function expression se ignora sin error. Solo
  diverge en módulo (TypeError).
- `oxc-o2-typeof` (texto de ENVIAR): `number` contra `function` en A (Stdout) y TypeError en B.
- `oxc-n6422-2-log`: `PASS` contra `FAIL` (Stdout) en las 4 celdas de oxc.
- `oxc-n6422-2-fixture`: tiene `import`, así que en carril A es `parse_esm` (descarte). En B, AssertionError
  (ThrowNew) en oxc **y en swc**: `@swc/core` 1.16.2 también borra el spread con getter (verificado a mano).
- `oxc-o4`: `delete NaN` da `false` en sloppy (Stdout) y SyntaxError temprano en módulo (ThrowNew).
- `oxc-o5`: `0` contra `1` (Stdout) en las 4.
- `oxc-o6`: panic del minifier de oxc (también sin debug assertions).
- `oxc-o7`: debug_assert de oxc. Con el oráculo el minificado es el de npm y no diverge: `asserts/` en las 4 de oxc.
- `oxc-eval-indirecto` (R1-H05, `revision-1/pruebas/c6-corpus/eval-indirecto`): el debug_assert de oxc tapaba un
  miscompile. Con el oráculo, oxc cambia `var e = eval; e("x")` por `eval("x")`: `global/local` contra
  `local/local` (Stdout) en las 4 de oxc, con `solo_debug_assertions: si` en el meta. swc: igual.
- `swc-s1-use-strict`: swc inlinea una función estricta y pierde la directiva. Solo diverge en sloppy (carril A).
- `swc-s3-neg-zero-member`: swc imprime `-0[-0]=0` (SyntaxError) solo con el preset carril_a en A. En B el original ya
  lanza TypeError (propiedad sobre un primitivo en estricto), y por eso es ThrowType en los dos presets.
- `oxc-cbrt-stderr` (11.1, repro `s8` de la revisión integral 1): el bug E de oxc (`Math.cbrt(-8)` → `NaN`) escrito
  solo con `process.stderr.write`. El preload lo pasa a stdout (`[stderr] -2` contra `[stderr] NaN`): Stdout en las
  4 de oxc. swc no tiene el bug: igual.
- `oxc-cbrt-exit1` (11.2, repro `s6` de la revisión integral 1): el mismo bug impreso por stdout en un programa que
  sale con `process.exit(1)` a propósito. El original con un código propio se compara (antes: descarte `exit`):
  Stdout en las 4 de oxc. swc: igual.
- `difuzz-senal-b` (11.4, como el repro `senal/q1-b-muere` de la revisión integral 1): no es un bug del minifier sino
  una prueba del oráculo. El programa lee su propio texto (`process.argv[1]`) y se mata con SIGSEGV si ya no tiene el
  nombre de su variable local: el original impreso lo tiene; el minificado no (mangle de los nombres locales o
  inlining, en las 8 celdas). Solo B muere por señal: ExitStatus en las 8. Con `Signal => Same` en `oracle::compare`
  (la mutación m3 de la revisión) daba igual.
- `control-limpio`: igual en todas.

## Historia

- 2026-10-07 (binario de `9f100ab`): 120/120 en las celdas base. Fallaba 1 comparación de modo:
  `oxc-b-core_defaults --invocar` perdía `oxc-n6422-2-fixture` (H01 de la revisión 2).
- b2-int (2026-10-08): layout `<out>/<celda>/` y `CORRIDA.txt` completa; la identidad de stats suma `RECURSOS`
  (b2-b: timeout/memoria del minificador van a `recursos/`, destino terminal como PANICS); los descartes
  `analisis` (b2-c) ya son filas de descartes.tsv; `clase_de` reconoce `recursos/`. MEDIDO: 160/160.
- b5 (2026-10-09): pasa al repo (`tests/golden/`, sin rutas absolutas). `clase_de` reconoce `asserts/` (antes
  contaba como `igual`): `oxc-o7` pasa de `igual` a `asserts`. Caso nuevo `oxc-eval-indirecto`. Opciones para
  reutilizarlo con otro corpus. MEDIDO con el binario de `46e6749`: 172/172 (128 de celdas base + 44 de modos),
  unos 15 s con `--jobs 2`.
- 11a (2026-10-13): 3 casos nuevos de la revisión integral 1: `oxc-cbrt-stderr` (11.1), `oxc-cbrt-exit1` (11.2) y
  `difuzz-senal-b` (11.4). MEDIDO con los binarios de `5e55637` y `c780b2e`: 208/208 (152 de celdas base + 56 de modos). Con el
  binario base (`2ff4d90`) los tres terminan en `igual` (el primero), `descarte:exit` y `div:ExitStatus`; con
  `Signal => Same` (mutación m3) `difuzz-senal-b` falla en las 8 celdas.
