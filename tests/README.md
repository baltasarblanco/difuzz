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
- `tests/salida_usuario.rs`: códigos de salida 0, 1 y 2 de difuzz, que `descartes.tsv` reconstruya el total y que un pipe cerrado en stdout o stderr no cambie el código ni pierda una divergencia (9.4).
- `tests/verificaciones.rs`: sus dos tests (golden y aceptación) quedan ignorados; ver "Como tests de cargo".

Cada binario de integración incluye `src/test_util.rs` (con `#[path]`) y corre también sus 4 tests. Ignorados en
total: 3, los dos de `tests/verificaciones.rs` y C1 de la muestra (`c1_igual_a_oxc_minify_de_npm`, en `src/`).

Sin `node` en `PATH`, los tests que lo necesitan **fallan** con un mensaje claro (R1-H17). Con
`DIFUZZ_SIN_NODE=1` se saltean y cada uno deja una línea `DIFUZZ_SALTEADO_SIN_NODE` en stderr (contarlas con
`cargo test --no-fail-fast 2>&1 | rg -c '^DIFUZZ_SALTEADO_SIN_NODE'`). Ver `src/test_util.rs`.
Lo mismo con los tests de `--sandbox-red` (`src/sandbox_tests.rs`), que necesitan *user namespaces*: si la
máquina no los arma, **fallan** (7.5; antes pasaban sin probar nada); `DIFUZZ_SIN_USERNS=1` los saltea y cada uno
deja una línea `DIFUZZ_SALTEADO_SIN_USERNS` en stderr.

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

159 casos que combinan un bug real de oxc con un flag de asunción (o con la forma en que se imprime), en las 4
celdas de oxc (159 × 4 = 636 comparaciones): 21 de la revisión 1 (C4, `00-` a `10-`), 9 adversariales del revisor de la fase R (`r01` a
`r10`, sin `r05`), `14-tdz-positivo`, 11 de los flags que antes no tenían re-chequeo (`15-` coerción, `16-`
built-ins, `17-` contenido de `toString`/`.stack`, `18-` profundidad: positivos y adversariales con el Issue A),
15 de la revisión de t-int (`19-`: el bug adentro del código que el re-chequeo neutraliza; r03, r06 y r13 son
controles), 5 del paso 7 (`20-`), 23 de n-principio (`21-`: `extends`, `with`, `global_conocido`,
violaciones y solo sloppy con el bug en la rama que solo corre con la violación; la TDZ en un `return`; y sus
positivos), 5 de `.stack` (`22-`, de n-ruido: el bug cambia nombre, cuenta u orden de los marcos), 5 del
tope del sensor (`23-t`, ítem 7.4: 1000 excepciones atrapadas y una TDZ atrapada), 13 del valor por defecto
(`23-b71-`, ítem 7.1: B == A' porque el bug deja justo el valor que la variable tiene cuando la rama de la
violación no corre; el re-chequeo mira además el camino de B), 1 de los timers opacos (`23-b01-timer-repeat`,
ítem 7.3: el bug solo se ve en `_repeat` de un timer cancelado), 3 de la revisión de la integración (`23-i01`,
`23-i02`, `23-i03`: el programa nota el sensor por su `Error.prepareStackTrace` y solo sin él corre el bug; en
`23-i03`, solo en el lado derecho de un `&&=`), 15 de 7.1b (`23-b71b-`: lados de `&&=`/`||=`/defaults que la
cobertura no separa, dependencia por datos, y `q02t`, la línea con forma de marco elegida por datos), 10 de 7.1c (`23-b71c-`: el texto de las funciones con los
contadores, la sal que cambia el camino y un programa que pisa lo que usa el preload) y 2 del sensor
omitido (`23-b78-`, ítem 7.8: el patrón del bug de V8 con el sensor, sloppy + `eval` directo + clase; en el carril A
corren sin el sensor y una TDZ atrapada que solo él vería va a `div/`, no a `known/`), 3 de la profundidad
con un desborde de pila nativo usado como dato (`24-f1-`, ítem 8.1: `d01` y `d04` van a `div/` porque el
`Explica` de profundidad se confirma salado; `p01`, el positivo, sigue en `known/`, pero depende del límite de
pila de este node), 6 del nombre local en un mensaje de V8 (`24-b83-`, ítem 8.3: `cb is not a function`
contra `e is not a function`; 3 positivos y 3 con un bug real al lado, en el código que el bug deja de correr
o dentro de la expresión que V8 escribe), 3 de H1 de la revisión del bloque 8 (`24-b83h1-`: con keep_names
oxc manglea un parámetro con default anónimo y el `.name` que el programa imprime con la forma del mensaje
cambia; van a `div/` en carril_a y a `known/` en core_defaults, donde keep_names está apagado; `n03` lee el
`.name` con una clave armada en runtime), 2 del sensor perezoso en B (`24-b87-`, ítem 8.7: una TDZ atrapada
por un `try` armado en runtime, que `runner::puede_atrapar` no ve en el minificado; con el bug en el catch van
a `div/`) y 6 de los nombres que imprime `util.inspect` (`25-b91-`, ítem 9.1: el mismo bug de keep_names que
`24-b83h1-`, visto como `[Function: handler]` contra `[Function: e]`, con clase, `bound`, privado y `catch`;
`c01` es el control con las formas que keep_names cubre). Los `19-`, `20-` y `22-` esperan `div/` en todas las
celdas; los `21-`, `23-` y `24-`, `div/` salvo los positivos (menos `24-b83-p02` en core_defaults, que sigue en
`div/`: el marco del stack cambia de nombre) y los `24-b83h1-` en core_defaults. Los `25-b91-p` esperan `div/`
en carril_a e igual en core_defaults (sin keep_names los nombres se borran); `25-b91-c01`, igual en las 4.
Destino esperado en `esperado.tsv` (mismo formato que el golden). Reutiliza `tests/golden/verificar.sh`.
Lo que la aceptación no cubre: un programa en que oxc pliega mal la condición y borra la rama entera de la
violación (B no depende de la asunción y no se distingue de un minificado que la aprovechó; ver "Límites
conocidos" en el README principal).

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
