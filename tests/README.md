# Tests de difuzz

Todo lo que corre difuzz de punta a punta necesita `node` en `PATH`. Lo que compara clasificaciones necesita
además el **oráculo sin debug assertions**: difuzz lo busca en `<binario>-oraculo`, en
`<dir del binario>/../oraculo/difuzz` (o sea `target/oraculo/difuzz`) o en `DIFUZZ_ORACULO=RUTA`.

```sh
cargo build --release && cargo build --profile oraculo   # los dos binarios
```

## Suite normal

`cargo test`: los unitarios de `src/` y tres binarios de integración:

- `tests/salida_celdas.rs`: dos celdas en el mismo `--out` no se pisan; una corrida cortada queda marcada.
- `tests/salida_usuario.rs`: códigos de salida 0, 1 y 2 de difuzz, y que `descartes.tsv` reconstruya el total.
- `tests/verificaciones.rs`: sus dos tests (golden y aceptación) quedan ignorados; ver "Como tests de cargo".

Cada binario de integración incluye `src/test_util.rs` (con `#[path]`) y corre también sus 3 tests. Ignorados en
total: 3, los dos de `tests/verificaciones.rs` y C1 de la muestra (`c1_igual_a_oxc_minify_de_npm`, en `src/`).

Sin `node` en `PATH`, los tests que lo necesitan **fallan** con un mensaje claro (R1-H17). Con
`DIFUZZ_SIN_NODE=1` se saltean y cada uno deja una línea `DIFUZZ_SALTEADO_SIN_NODE` en stderr (contarlas con
`cargo test --no-fail-fast 2>&1 | rg -c '^DIFUZZ_SALTEADO_SIN_NODE'`). Ver `src/test_util.rs`.

## Golden de detección — `tests/golden/`

16 casos con bugs conocidos de oxc y swc, en 8 celdas (swc|oxc × carril a|b × core_defaults|carril_a), más
`--invocar`/`--stubs` en oxc core_defaults. Clase esperada por caso y celda en `esperado.tsv`, justificada
en `tests/golden/README.md`.

```sh
tests/golden/verificar.sh [--jobs N] [BINARIO] [DIR_SALIDA]
```

`BINARIO` por defecto: `${CARGO_TARGET_DIR:-target}/release/difuzz`. Sale con 0 si todo coincide y deja
`comparaciones=N fallas=0` en la última línea.

## Aceptación de los re-chequeos — `tests/aceptacion/rechequeos/`

62 casos que combinan un bug real de oxc con un flag de asunción, en las 4 celdas de oxc (62 × 4 = 248
comparaciones): 21 de la revisión 1 (C4, `00-` a `10-`), 9 adversariales del revisor de la fase R (`r01` a
`r10`, sin `r05`), `14-tdz-positivo`, 11 de los flags que antes no tenían re-chequeo (`15-` coerción, `16-`
built-ins, `17-` contenido de `toString`/`.stack`, `18-` profundidad: positivos y adversariales con el Issue A),
15 de la revisión de t-int (`19-`: el bug adentro del código que el re-chequeo neutraliza; r03, r06 y r13 son
controles) y 5 del paso 7 (`20-`). Los `19-` y `20-` esperan `div/` en todas las celdas.
Destino esperado en `esperado.tsv` (mismo formato que el golden). Reutiliza `tests/golden/verificar.sh`.
Lo que la aceptación no cubre: un programa armado para que la salida con el bug coincida con la del original
neutralizado va a `known/` (ver "Límites conocidos" en el README principal).

```sh
tests/aceptacion/rechequeos/verificar.sh [--jobs N] [BINARIO] [DIR_SALIDA]
```

## Como tests de cargo

```sh
cargo test --release --test verificaciones -- --ignored --test-threads=1   # los dos
cargo test --release --test verificaciones -- --ignored golden             # solo el golden
```

Usan `CARGO_BIN_EXE_difuzz` (con `--release`, `target/release/difuzz`; sin `--release`, `target/debug/difuzz`,
que también encuentra `target/oraculo/difuzz`). Fallan con un mensaje claro si no hay node o no hay oráculo
(sin node, `DIFUZZ_SIN_NODE=1` los saltea y los cuenta, como al resto: ver "Suite normal").
`VERIFICAR_JOBS=N` fija los workers de difuzz (default 4); `--test-threads=1` evita correr los dos a la vez.

## Muestra fija — `tests/muestra/`

Ver `tests/muestra/README.md`.

## Arnés de no regresión — `scripts/comparar-binarios.sh`

Compara dos binarios caso por caso (destinos, `meta.txt`, archivos, stats, descartes) sobre un corpus, celda
por celda. Para un refactor que no debe cambiar nada:

```sh
scripts/comparar-binarios.sh --jobs 2 --corpus DIR BIN_BASE BIN_NUEVO DIR_SALIDA "oxc b"
```

El corpus sale de `--corpus DIR` o de la variable `DIFUZZ_CORPUS`; sin ninguno de los dos, el script sale con 2.
Para los fixtures de SWC: `<clon de swc>/crates/swc_ecma_minifier/tests`. `--help` lista las opciones
(`--ignorar-campo`, `--reusar`, `--base-de`, ...) y sale con 0. `scripts/matriz.sh`, `scripts/medir-serie.sh` y
`scripts/subir-oxc.sh` toman el corpus de `CORPUS` (o, si falta, de `DIFUZZ_CORPUS`).
