# difuzz

Fuzzer diferencial de minifiers de JavaScript, escrito en Rust. Para cada caso de un corpus,
minifica el programa, ejecuta el original y el minificado con `node` y compara lo que hacen. Si
difieren, el minifier cambió el comportamiento del programa: es un candidato a bug
(*miscompile*).

## Cómo funciona

1. **Corpus.** Todos los archivos `input.js` bajo el directorio que se le pasa, a cualquier
   profundidad (no sigue symlinks a directorios), en orden fijo.
2. **Minificar.** El backend parsea el caso una sola vez e imprime dos programas con el mismo
   printer: el original sin optimizar y el minificado. Así, una diferencia solo puede venir del
   optimizador, nunca del parser ni del printer. Por defecto minifica en un proceso hijo
   persistente por worker: un abort, un desborde de pila o un cuelgue del minifier no tiran
   abajo la corrida.
3. **Ejecutar.** Los dos programas corren con `node --require preload.cjs`, en un entorno mínimo
   y con `TZ=UTC`: como `.cjs` (script, sloppy) en el carril A o como `.mjs` (módulo ES) en el
   carril B. El *preload* se carga igual en los dos lados y:
   - fija lo no determinista (`Date`, `Math.random`, `performance.now`, `process.pid`, ...);
   - agrega un observador: a un programa que no imprime nada se le añade un epílogo que imprime
     sus bindings de nivel superior;
   - instrumenta las excepciones: un `throw` no atrapado imprime su tipo y sale con código 70;
   - hace de sensor de asunciones: marca con *flags* si el programa usa algo que el minifier
     declara asumido (contenido de `Function.prototype.toString` o de `.stack`, built-ins
     pisados, TDZ, ...);
   - manda `console.error`/`warn`/`trace` a stdout y, al salir, imprime los globals que el
     programa creó.
4. **Comparar.** Si el estado de salida o el stdout difieren, hay divergencia, de una de estas
   clases (`kind`):

   | kind | qué pasó |
   |---|---|
   | `Stdout` | mismo estado de salida, distinto stdout |
   | `ThrowNew` | el minificado lanza y el original no (p. ej. un `SyntaxError` introducido) |
   | `ThrowLost` | el original lanza y el minificado no |
   | `ThrowType` | los dos lanzan, con tipos distintos |
   | `ExitStatus` | el minificado sale con otro código o muere por señal |
   | `Timeout` | el minificado se cuelga y el original no |

5. **Clasificar.** Cada divergencia confirmada va a `div/` (hallazgo) o a `known/` (la explica
   una asunción documentada del minifier). Ver [Clasificación](#clasificación-div-o-known).

## Minifiers soportados

| `--minifier` | crate | versión fijada | |
|---|---|---|---|
| `oxc` | `oxc_minifier` (y el resto de `oxc_*`) | `=0.153.0` | por defecto |
| `swc` | `swc_ecma_minifier` | `61.0.8` (`Cargo.lock`) | |

Las versiones salen de `Cargo.toml` y de `Cargo.lock`, que está versionado. Cada `meta.txt`
registra la versión con la que se encontró el hallazgo (`backend: oxc_minifier 0.153.0`).

Cada minifier tiene dos presets (`--config`):

- `core_defaults`: lo que ejecuta el paquete npm con las opciones por defecto. oxc:
  `minifySync` de `oxc-minify` sin opciones. swc: `minifySync(src, { compress: {}, mangle: {} })`
  de `@swc/core`. Es el uso común.
- `carril_a` (por defecto): lo mismo con los nombres de funciones y clases preservados
  (`keep_*`) y el mangle de nivel superior encendido; en swc además el DCE de nivel superior y
  `compress.module = false` (sloppy, como node al ejecutar un `.cjs`).

En los presets de oxc las anotaciones `/*@__PURE__*/` y `/*#__NO_SIDE_EFFECTS__*/` están
desactivadas (`treeshake.annotations = false`): oxc las honra por contrato y borrar una llamada
anotada que sí tiene efectos no es un bug. Es la única diferencia con `minifySync`.

## Requisitos

- Rust y cargo. Verificado con rustc 1.98.0; el crate no declara `rust-version`.
- `node` en el `PATH`. Verificado con Node v24.14.1; no hay versión mínima declarada.
- Linux. difuzz usa `/proc` y `prctl(PR_SET_CHILD_SUBREAPER)` para no dejar procesos
  huérfanos, y lanza sus procesos hijos por `/proc/self/exe` (el oráculo, por un descriptor
  abierto al arrancar), así que en `ps` aparecen con esos nombres. En otros sistemas no está
  verificado.
- Para los scripts de `tests/` y `scripts/`: bash y `rg`, `fd`, `eza` y `sd` (cada script lista
  sus dependencias en la cabecera).

## Compilar

```sh
cargo build --release && cargo build --profile oraculo
```

Deja dos binarios:

- `target/release/difuzz`: el que se usa. Lleva *debug assertions* encendidas
  (`[profile.release]` en `Cargo.toml`) porque los diagnósticos internos de los minifiers solo
  existen con ellas.
- `target/oraculo/difuzz`: el **oráculo**, el mismo código sin debug assertions, como el paquete
  npm. Si el minifier hace panic en un `debug_assert!`, difuzz vuelve a minificar ese caso con el
  oráculo: si el oráculo minifica, el caso se compara con ese minificado (va a `div/` con la nota
  `solo_debug_assertions`, o a `asserts/` si no diverge); si el oráculo también falla, va a
  `panic/`. Así un assert de desarrollo no tapa un miscompile que npm sí produce.

difuzz busca el oráculo en `<binario>-oraculo` y en `<dir del binario>/../oraculo/difuzz`, o solo
en `DIFUZZ_ORACULO=RUTA` si está definida. Al arrancar dice cuál usa; si no encuentra ninguno,
avisa y los panics van a `panic/`. Hay que recompilar los dos binarios juntos.

En Linux, recompilar a mitad de una corrida no la afecta: difuzz lanza sus hijos por
`/proc/self/exe` y el oráculo por el archivo que abrió al arrancar, así que reemplazar
cualquiera de los dos por otro archivo no cambia lo que se ejecuta. Reescribir el oráculo en el
lugar (mismo archivo; un `touch` también cuenta) sí corta la corrida (código 1). Sin `/proc`, los
lanza por ruta y cualquier cambio de los dos la corta.

## Uso

```sh
difuzz <corpus_dir> [opciones]    # correr un corpus
difuzz --emitir ARCHIVO [...]     # imprimir original y minificado de un archivo, sin node
difuzz --cruce DIR                # tabla de cruce de los índices de varias celdas
difuzz --help
```

Ejemplos con el corpus chico que trae el repo:

```sh
B=target/release/difuzz
$B tests/golden/corpus --out salida --jobs 4                           # oxc, carril_a, carril A
$B tests/golden/corpus --out salida --minifier swc --carril b --config core_defaults
$B --emitir tests/golden/corpus/oxc-o5/input.js
```

Para un corpus grande, los tests del minifier de SWC (`crates/swc_ecma_minifier/tests` en un
clon de SWC) son la fuente de casos con la que se desarrolló difuzz.

Opciones principales (la lista completa, con defaults, está en `difuzz --help`):

| opción | qué hace | default |
|---|---|---|
| `--out DIR` | dónde escribir; cada celda en `DIR/<celda>/` | `./difuzz-out` |
| `--minifier M` | `oxc` \| `swc` | `oxc` |
| `--config C` | preset: `carril_a` \| `core_defaults` | `carril_a` |
| `--carril K` | `a` (script, `.cjs`) \| `b` (módulo ES, `.mjs`) | `a` |
| `--forma F` | `tal_cual` \| `en_funcion` (envuelve el caso en una función) | `tal_cual` |
| `--jobs N` | workers en paralelo | núcleos disponibles |
| `--timeout-ms N` | timeout por ejecución de node | 5000 |
| `--limit N` | solo los primeros N casos | |
| `--invocar` | el observador además llama a las funciones observadas | |
| `--mutaciones N` | N variantes por caso (borrar sentencias, perturbar literales) | 0 |
| `--mutaciones-extra N` | N variantes con los operadores nuevos | 0 |
| `--stubs` | un `import`/`require` que no resuelve pasa a un módulo inerte en vez de descartar el caso | |
| `--cache DIR` / `--no-cache` | caché de la ejecución del original, compartible entre celdas | `<out>/cache-a` |
| `--indice DIR` | anotar cada hallazgo en `DIR/index-<celda>.tsv` | |
| `--en-proceso` | minificar en el proceso de difuzz, sin hijo | |
| `--mem-hijo MiB` | presupuesto de memoria de los hijos de cada worker | MemTotal/2 / workers |

## Celdas

Una **celda** es una combinación de opciones que cambia lo que se mide. Su nombre es
`<minifier>+<config>+<forma>`, seguido, si aplican, de `+carril_b`, `+invocar`, `+mut<N>`,
`+mutx<N>` y `+stubs`, en ese orden. Por ejemplo:

```
oxc+carril_a+tal_cual                    (los defaults)
swc+core_defaults+tal_cual+carril_b      (--minifier swc --config core_defaults --carril b)
oxc+carril_a+en_funcion+invocar          (--forma en_funcion --invocar)
oxc+core_defaults+tal_cual+mut2+stubs    (--config core_defaults --mutaciones 2 --stubs)
```

Celdas distintas con el mismo `--out` no se pisan. Volver a correr una celda reemplaza solo lo
suyo.

## Salida

```
<out>/
├── cache-a/                   caché de las ejecuciones del original (salvo --no-cache)
├── lint-triage.tsv            triage manual del lint (si hubo sospechosos)
└── <celda>/
    ├── CORRIDA.txt            estado de la corrida, comando, versiones y stats
    ├── descartes.tsv          casos que no se compararon: caso, causa, elapsed_ms
    ├── known-sintomas.tsv     síntoma registrado de cada conocida
    ├── preload.cjs            el preload, para reproducir a mano
    ├── div/<caso>/            divergencias: hallazgos
    ├── known/<caso>/          divergencias explicadas por una asunción documentada
    ├── panic/<caso>/          el minifier hizo panic (también sin debug assertions)
    ├── asserts/<caso>/        panic solo con debug assertions y sin divergencia
    ├── recursos/<caso>/       el minifier no terminó en 60 s o pasó el presupuesto de memoria
    └── lint/<caso>/           sospechosos del lint: el minificado escribe sobre literales nuevos
```

Los directorios de destino se crean solo si algún caso termina ahí. `<caso>` es la ruta del caso
relativa al corpus, sin `input.js` y con `/` cambiado por `__` (`terser/compress/x/input.js` →
`terser__compress__x`).

- **`CORRIDA.txt`**: la primera línea es `estado: completa`, `incompleta` o `interrumpida`.
  Solo `completa` es una corrida terminada; con otro estado, `div/`, `descartes.tsv` y el resto
  pueden estar a medias.
- **Stats.** Al terminar, difuzz imprime en stdout una línea `total=... iguales=...
  DIVERGENCIAS=... conocidas=... PANICS=... RECURSOS=... | descartados: ...` (la misma queda en
  `CORRIDA.txt`). Cada caso termina en un solo destino, así que, sin `--mutaciones`:

  ```
  total = iguales + DIVERGENCIAS + conocidas + PANICS + RECURSOS + filas de descartes.tsv
  ```

  `lint/` y `asserts/` son ortogonales: el caso cuenta también donde terminó.
- **`--indice DIR` y `--cruce DIR`.** Con `--indice`, cada celda deja `DIR/index-<celda>.tsv`
  al terminar (una corrida interrumpida deja intacto el índice anterior). `difuzz --cruce DIR` lee esos índices y, sin correr
  nada, imprime una tabla caso × celda con el `kind` de cada hallazgo (`S`=Stdout,
  `N`=ThrowNew, `P`=panic, ...) y los totales por celda.

## Cómo leer un hallazgo

Cada `div/<caso>/` (y `known/<caso>/`) trae:

- `original.cjs` y `minified.cjs` (`.mjs` en el carril B): los dos programas que se ejecutaron;
- `a.stdout`, `a.stderr`, `b.stdout`, `b.stderr`: lo que imprimió cada uno (`a` es el original,
  `b` el minificado), y `a.exc`/`b.exc` con el mensaje y el stack si alguno lanzó;
- `meta.txt`: `kind`, celda, flags del sensor, estado y tiempo de cada lado, versiones de node y
  del minifier, y dos comandos listos para reproducir cada lado (`repro_original:` y
  `repro_minificado:`). Al final, la sección `clasificacion:` con un paso por línea y la
  decisión:

  ```
  clasificacion:
    profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)
    asunciones: ninguna (ni flag de la tabla, ni ruta, ni backlog)
    solo_sloppy: NoExplica (con "use strict" sigue)
    decision: div/
  ```

Para confirmar un hallazgo: correr los dos `repro_*` del `meta.txt` (usan el `preload.cjs` de la
celda), reducir el caso a mano y reproducirlo con el paquete npm del minifier
(`difuzz --emitir` muestra el minificado de un archivo sin ejecutar nada). `panic/`,
`recursos/` y `asserts/` traen el `input.js` y el mensaje en `meta.txt`.

## Clasificación: `div/` o `known/`

Un minifier documenta **asunciones** sobre el código que recibe; un programa que las viola puede
cambiar de comportamiento sin que sea un bug. Cada backend declara las suyas
(`src/backend/oxc.rs`, `src/backend/swc.rs`):

- **oxc**: las de `crates/oxc_minifier/docs/ASSUMPTIONS.md` del repositorio de oxc (verificadas
  contra el tag `crates_v0.153.0`). Además, difuzz trata el modo sloppy como no soportado por
  oxc: en el carril A, una divergencia sin otra explicación que desaparece al agregar
  `"use strict"` va a `known/`.
- **swc**: las del `AGENTS.md` del minifier de SWC, más su backlog (`TODO.txt` y
  `postponed.txt` en la raíz del corpus de tests de SWC).

Principios:

- **Una divergencia va a `known/` solo si sus flags están en la tabla de asunciones del backend
  y el re-chequeo de esos flags (si lo tienen) da `Explica`.** El re-chequeo neutraliza la
  violación (con otra ejecución o volviendo a minificar): A' es el original y B' el minificado así
  neutralizados. Solo `Explica` conserva el flag; con `NoExplica` (la divergencia sigue) o
  `Inconcluso`, el flag se renombra (`extends_irrelevante`, `extends_inconcluso`, ...), ya no
  clasifica y, si no hay otro, el caso queda en `div/` con una nota. Qué cuenta como `Explica`:
  - contenido de `toString`/`.stack` (A' y B' con el contenido fijo), profundidad de pila (A' y
    B' con más pila), built-ins pisados (A' y B' con lo pisado congelado en su valor original) y
    métodos de coerción (se renombran las definiciones de `valueOf`/`toString`/
    `[Symbol.toPrimitive]` y se vuelve a minificar), en los dos backends: la neutralización hace
    que el código donde vive la asunción no corra, así que B' == A' no alcanza (si el bug está
    ahí adentro, A' y B' lo saltean los dos). Hace falta además que B dé lo mismo que A' (el
    minificado de verdad se comporta como el original neutralizado). En el contenido también
    vale que B difiera de A solo en líneas que muestran el contenido (en A' el texto fijo o un
    marco fijo, y en A el contenido de verdad); un marco solo puede cambiar la posición
    (`archivo:L:C`): otro nombre, otro orden u otra cantidad de marcos es otro camino.
  - `extends` (el heredero literal pasa a `null`), `with` (sin el scope), `global_conocido`
    (stubs), las violaciones que ubica el sensor (TDZ, atrapada o no; coerción de `Symbol`;
    `extends` atrapado: cada una pasa a algo que no lanza) y solo sloppy (`"use strict"`), en
    oxc: la misma vara, B' == A' y B == A'. Acá la neutralización no apaga el código de la
    asunción pero cambia el camino: lo que solo corre CON la violación (el `catch` de la TDZ, la
    rama sin `window`, la rama sloppy) no corre en A' ni en B', y un bug ahí iba a `known/`
    cuando alcanzaba con que A' diera lo mismo que A o que B difiriera solo en líneas que A'
    también cambia (medido con oxc 0.153.0: 17 programas, 116 de 264 divergencias en 8 celdas).
    Una TDZ que V8 ubica en un `return`/`throw` (no en el identificador) se neutraliza si la
    sentencia tiene un solo identificador ligado con `let`/`const`/`class`; si no, inconcluso.
    Para un original que liga `arguments` (no compila en estricto), primero se prueba el
    renombre solo: si A con el binding renombrado da lo mismo que A y su minificado también,
    la divergencia la explica ligar `arguments`, válido solo en sloppy.
  - los nombres (minificar preservándolos), en los dos: que la divergencia desaparezca.

  En oxc todos los flags de su tabla tienen re-chequeo. En swc, `proxy`, `tdz`, `callee`,
  `redefine_global`, el backlog y la asunción por ruta (`issue_2719`) clasifican directo, sin
  re-chequeo.

  **Límites conocidos** (lo que no se prueba): el re-chequeo compara salidas, no caminos. Un
  programa armado para que la salida con el bug coincida justo con la del original neutralizado
  (o que imprima a propósito una línea con forma de marco de V8 en la rama del contenido fijo)
  termina en `known/` (los dos casos se midieron así con oxc 0.153.0). Un bug que está en el
  programa pero que B no ejecuta (oxc borró la sentencia que lanza y con eso el `catch` que lo
  tenía) no deja rastro en la salida: la divergencia es entera de la asunción y va a `known/`.
  Lo que se pierde por exigir B == A' (van a `div/`): los positivos en que B no da ni lo de A
  ni lo de A' (con `with`, terser `issue_1105/assorted_*`: oxc pasa `0/0` a `NaN`, que adentro
  del `with` resuelve a la propiedad del objeto; en sloppy, terser `dead_code_2_should_warn` y
  `dead_code_constant_boolean_should_warn_more`: funciones en bloque de Anexo B), y un `.stack`
  impreso en que el minificado cambia el nombre de un marco (el mangling de `core_defaults`) o
  la cantidad de marcos (inlining).
- **Un minificado que node no acepta nunca se explica con una asunción**: ninguna permite
  producir código inválido.
- **Una conocida que cambia de síntoma vuelve a `div/`** (se compara contra
  `known-sintomas.tsv`).

La sección `clasificacion:` de `meta.txt` muestra cada re-chequeo y su resultado.

## Códigos de salida

| código | significado |
|---|---|
| 0 | corrida entera (con o sin hallazgos; `CORRIDA.txt: completa`); `--cruce`, `--emitir` y `--help` sin error |
| 1 | error de entrada o de entorno: corpus inexistente, que no es directorio o sin `input.js`; `--out` que no se puede escribir; escrituras perdidas o casos sin contar; el oráculo se reescribió a mitad de corrida (ver [Compilar](#compilar)); `--cruce` sin índices; `--emitir` que no puede leer o minificar |
| 2 | error de uso: opción desconocida, valor o combinación inválida (con la ayuda en stderr) |
| 130 | interrumpida por SIGINT o SIGTERM (`CORRIDA.txt: interrumpida`) |

## Tests y verificaciones

Detalle en [`tests/README.md`](tests/README.md). Lo que corre difuzz de punta a punta necesita
`node`; el golden y la aceptación necesitan además el oráculo (los dos binarios compilados).

```sh
cargo test                                            # unitarios y de integración
DIFUZZ_SIN_NODE=1 cargo test --no-fail-fast           # sin node: saltea y cuenta los que lo necesitan
```

Sin `node` en el `PATH`, los tests que lo necesitan fallan con un mensaje claro. Con
`DIFUZZ_SIN_NODE=1` se saltean y cada uno deja una línea `DIFUZZ_SALTEADO_SIN_NODE` en stderr.

- **Golden de detección** (`tests/golden/`): 16 casos con bugs conocidos de oxc y swc, en 8
  celdas, más `--invocar` y `--stubs`; la clase esperada de cada caso está en `esperado.tsv`.
- **Aceptación de los re-chequeos** (`tests/aceptacion/rechequeos/`): 90 casos que combinan un
  bug real de oxc con un flag de asunción, en las 4 celdas de oxc (360 comparaciones).

```sh
tests/golden/verificar.sh --jobs 4                    # sale con 0 y "comparaciones=N fallas=0"
tests/aceptacion/rechequeos/verificar.sh --jobs 4
cargo test --release --test verificaciones -- --ignored --test-threads=1   # los dos, como tests de cargo
```

- **Muestra fija** (`tests/muestra/`): 100 casos del corpus de tests de SWC para auditar el
  printer y el original. `cargo test muestra::` corre A1 y A2 sin node. C1 compara el minificado
  de oxc con `oxc-minify` de npm y necesita `npm ci` en `probe/`:
  `cargo test muestra::c1 -- --ignored --nocapture`. Origen y criterio de selección en
  [`tests/muestra/README.md`](tests/muestra/README.md).
- **Arnés de no regresión** (`scripts/comparar-binarios.sh`): corre un corpus con dos binarios,
  celda por celda, y compara caso por caso destinos, `meta.txt`, archivos, stats y descartes.
  Para un refactor que no debe cambiar ningún resultado:

  ```sh
  scripts/comparar-binarios.sh --jobs 4 --corpus tests/golden/corpus BIN_BASE BIN_NUEVO DIR_SALIDA "oxc b" "swc a"
  ```

  Sale con 0 sin diferencias, 1 con diferencias, 2 si no pudo comparar. El corpus sale de
  `--corpus DIR` o de la variable `DIFUZZ_CORPUS`; sin ninguno de los dos sale con 2 (los scripts
  de `scripts/` no traen un corpus por defecto). `--help` lista las opciones (`--ignorar-campo`,
  `--reusar`, `--base-de`, ...) y sale con 0.

Los scripts `scripts/matriz.sh` y `scripts/medir-serie.sh` (y `subir-oxc.sh`, abajo) toman el
corpus de la variable `CORPUS` o, si falta, de `DIFUZZ_CORPUS`; sin ninguna de las dos salen con
2. `matriz.sh` y `medir-serie.sh` no son ejecutables en el repo: se corren con `bash`.

## Subir la versión de oxc

`scripts/subir-oxc.sh VERSION DIR_SALIDA` (procedimiento en
[`scripts/subir-oxc.md`](scripts/subir-oxc.md)) sube los crates `oxc_*` y `oxc-minify` de
`probe/` a la vez, en un worktree temporal y sin commitear: compila los dos binarios de cada
versión, corre tests, golden, aceptación y el arnés, y deja un informe de qué hallazgos aparecen,
desaparecen o cambian, más un parche para aplicar. Tarda unos 20-30 minutos y necesita un
corpus en la variable `CORPUS` (o `DIFUZZ_CORPUS`; el de tests de SWC es
`<clon de swc>/crates/swc_ecma_minifier/tests`); sin corpus sale con 2 antes de crear nada. Con
`RED=1` además instala `oxc-minify` de npm y corre C1; necesita `REPRO=DIR` (sin ella sale con
2). Las variables y sus defaults están en `scripts/subir-oxc.sh --help`. Es retomable: ante un
corte, el mismo comando sigue desde el primer paso sin terminar.

## Licencia

difuzz todavía no tiene un archivo de licencia propio ni un campo `license` en `Cargo.toml`:
queda pendiente de que lo defina el dueño del repositorio.

Los casos de `tests/muestra/casos/` son archivos del proyecto
[SWC](https://github.com/swc-project/swc) copiados sin modificar, bajo licencia Apache 2.0
("Copyright 2024 SWC contributors"); el texto completo está en
[`tests/muestra/LICENSE-SWC`](tests/muestra/LICENSE-SWC) y el commit de origen en
`tests/muestra/README.md`.
