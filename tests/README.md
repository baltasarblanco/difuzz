# Tests de difuzz

Todo lo que corre difuzz de punta a punta necesita `node` en `PATH`. Lo que compara clasificaciones necesita
además el **oráculo sin debug assertions**: difuzz lo busca en `<binario>-oraculo`, en
`<dir del binario>/../oraculo/difuzz` (o sea `target/oraculo/difuzz`) o en `DIFUZZ_ORACULO=RUTA`.

```sh
cargo build --release && cargo build --profile oraculo   # los dos binarios
```

## Suite normal

`cargo test` (unitarios en `src/`, `tests/salida_celdas.rs`). Los tests de `tests/verificaciones.rs` quedan
ignorados.

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

21 casos (revisión 1, C4) que combinan un bug real de oxc con un flag de asunción, en las 4 celdas de oxc.
Destino esperado en `esperado.tsv` (mismo formato que el golden). Reutiliza `tests/golden/verificar.sh`.

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
scripts/comparar-binarios.sh --jobs 2 BIN_BASE BIN_NUEVO DIR_SALIDA "oxc b"
```

El corpus por defecto es el de los fixtures de SWC en `~/swc-main` (ruta absoluta en el script); `--corpus DIR`
lo cambia. `--help` lista las opciones (`--ignorar-campo`, `--reusar`, `--base-de`, ...).
