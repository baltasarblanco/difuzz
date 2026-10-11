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
| `--sandbox` | node con su modelo de permisos (ver [Sandbox](#sandbox)) | apagado |
| `--sandbox-red` | `--sandbox` más namespaces de Linux: sin red (ver [Sandbox](#sandbox)) | apagado |

## Sandbox

Por defecto node corre **sin sandbox**: el programa del corpus puede leer y escribir lo que
pueda el usuario que corre difuzz, lanzar procesos y usar la red. Con un corpus que no es de
confianza (paquetes de npm, código real) eso es un riesgo.

- **`--sandbox`**: cada ejecución de node (original, minificado, confirmaciones, re-chequeos,
  `--invocar`, envoltorio de módulos, stubs) corre con el modelo de permisos de node 24:
  `--permission`, `--allow-fs-read` solo para el preload, el programa (y el envoltorio de
  módulos) y el directorio de trabajo del caso, `--allow-fs-write` solo para ese directorio y
  el canal de control de difuzz, y `--allow-inspector` (con `--disable-sigusr1`) cuando corre el
  sensor de violaciones o la cobertura de un re-chequeo (ver abajo).
  Sin `--allow-child-process`, `--allow-worker`, `--allow-addons` ni `--allow-wasi`: no hay
  procesos hijos, workers, addons nativos ni WASI (tampoco `process.binding`). Como el modelo de
  permisos no cubre las señales ni la prioridad de otros procesos, un filtro seccomp (sin root;
  solo Linux x86_64 y aarch64: en otra arquitectura `--sandbox` no arranca y difuzz sale con 1)
  deja que node se mande señales solo a sí mismo (su pid, 0 o su grupo de procesos):
  `process.kill`, `process._kill` y `process._debugProcess` a cualquier otro proceso dan `EPERM`
  (`pidfd_send_signal`, siempre); y que cambie solo su propia prioridad: `os.setPriority` a otro
  proceso da `EPERM` (y cualquier `setpriority` sobre todos los procesos del usuario). Las
  llamadas de las señales y de la prioridad son lo único que el filtro mira: lo que llega por un
  socket (D-Bus, por ejemplo) no pasa por ellas; ver los límites.
- **`--sandbox-red`** (solo Linux, sin root, con *user namespaces*): `--sandbox` más un
  namespace de usuario (el mismo uid y gid adentro), uno de red (solo un loopback propio: un
  programa que se conecta a sí mismo por 127.0.0.1 anda; lo de afuera da `ENETUNREACH`) y uno
  de montajes, con `/run` tapado por un tmpfs y cada socket unix del sistema que esté fuera de
  `/run` (de `/proc/net/unix`, al lanzar cada node) tapado con `/dev/null`. Si no se puede
  armar, difuzz sale con 1 antes de empezar.

**`process.pid` y `process.ppid`** (con o sin sandbox). El preload los fija (para que el original
y el minificado vean lo mismo) en valores que ningún proceso puede tener: 4204242 y 4204241, más
que el máximo de Linux de 64 bits (`PID_MAX_LIMIT`, 4194304). Además envuelve `process._kill` (lo
que llama `process.kill`): una señal al pid ficticio (o a su grupo, el negativo) va al node real,
así que matarse, la señal 0 a sí mismo y un handler propio andan como en node y, con el sandbox,
no lo tocan; una señal al ppid ficticio da `ESRCH` sin llamar al sistema (nunca llega al padre
real, que es difuzz). Lo demás que reciba el pid ficticio no llega a ningún proceso:
`process._debugProcess` y `os.getPriority`/`os.setPriority` dan `ESRCH`, y `/proc/<pid>`,
`ENOENT`. Para el programa, `process.kill` y `process._kill` se ven como los de node (código,
`length`, `name`, errores). Sin el sandbox, una señal a un pid escrito a mano sigue llegando a
ese proceso, como cualquier otra cosa que el programa haga sin el sandbox.

**Simetría.** Original y minificado corren siempre con el mismo sandbox (mismo directorio de
trabajo, mismas rutas, mismos permisos): lo que se niega, se niega igual a los dos. Por eso un
caso que toca el sandbox (un permiso negado, atrapado o no; una señal o una prioridad negada; con
`--sandbox-red`, además, un error de red) **se compara igual**, toque uno solo o los dos, en
cualquier pasada (cruda, observada, de bloques, `--invocar`, envoltorio de módulos) y en las
variantes de `--mutaciones`. Si el original lo tocó, el minificado corre antes para saber si
también lo toca (y se compara ese mismo minificado, sin correrlo otra vez), y el hallazgo lleva
una nota en `meta.txt`:

- **los dos** (`sandbox: el original y el minificado tocaron el sandbox: se compara`): un programa
  que, por ejemplo, intenta leer un archivo, lo atrapa y sigue, y además tiene un bug del minifier
  que no tiene nada que ver con eso, va a `div/` como sin sandbox. Antes de 7.9 iba entero a
  `descartes.tsv` y el bug se perdía (revisión de `fe0cd86`, MENOR 3). Que esto no inventa
  divergencias se midió con 45 programas sin bug que tocan el sandbox de muchas formas (mensajes
  de error enteros con rutas, `util.inspect` y `.stack` del error, errores sin atrapar y con
  `cause`, `require` e `import()` de rutas negadas, procesos hijos, workers, `process.binding`,
  addons, WASI, señales, carril B y `--invocar`; red, DNS y `fetch` con `--sandbox-red`), en oxc y
  swc, 3 veces cada uno: ninguno fue a `div/` por el sandbox. Uno que imprime `e.stack` va a `div/`
  en `core_defaults` igual que sin sandbox (lo cambian los nombres acortados, no el sandbox).
- **solo el original** (`sandbox: solo el original toco el sandbox`): con el bug, el minificado no
  llega a abrir el archivo. Es un cambio de comportamiento.

Si lo toca solo el minificado (el minifier cambió qué archivo abre), se compara sin nota. Solo va a
`descartes.tsv` con causa **`sandbox`** el caso cuyo original tocó el sandbox y cuyo minificado no
pudo correr (falla del runner al lanzarlo: no se sabe si lo toca ni hay con qué comparar); en las
variantes, eso cuenta en `original=`. La celda lleva `+sandbox` o `+sandbox_red`, la caché de A no
se comparte con las corridas sin sandbox y al terminar se imprime (y queda en `CORRIDA.txt`) la
línea `sandbox: modo=... descartados=N solo_original=M ambos=K` (`N`: casos descartados por eso;
`M` y `K`: pasadas en que solo el original, o los dos, tocaron el sandbox y se compararon, también
las de las variantes de `--mutaciones`; en el carril B, un módulo mudo puede sumar dos: la pasada
cruda y la del envoltorio de módulos).

Cómo se reconoce que una ejecución tocó el sandbox: el preload anota `ERR_ACCESS_DENIED`,
`ERR_DLOPEN_DISABLED`, el `EPERM` de `kill` y el de `os.setPriority` (`uv_os_setpriority`) en su
canal cuando ve el error (no atrapado, en `--invocar` o, con el sensor de violaciones, atrapado
por el programa); con `--sandbox`, además, el envoltorio de `process._kill` anota el `EPERM` del
`_kill` de node (que no lanza: devuelve `-1`) y el de `process._debugProcess`, aunque el programa
los atrape. Con el sandbox, `os.setPriority` y `process._debugProcess` con el pid ficticio dan
`EPERM` (el filtro niega cualquier pid que no sea el real) y marcan. difuzz busca esos textos (y
`kill EPERM` y `uv_os_setpriority returned EPERM`) en la salida. Un `EPERM` natural se ve igual: un
`kill`/`_kill` a un proceso de otro usuario (el pid 1, por ejemplo) da `EPERM` también sin el
sandbox, y con `--sandbox` marca el caso: si el original y el minificado lo hacen, se compara con
la nota de A y B (antes de 7.9 iba a descartes y un bug real al lado no se veía; MEDIDO en la
revisión de la integración del bloque 7).

**Límites** (lo que el sandbox no frena):

- El modelo de permisos de node **no es una barrera contra código hostil**: **no frena la
  red** ni `connect` a un **socket unix** (medido con node 24.14.1): con `--sandbox`
  solo, un programa puede llegar a D-Bus, a systemd-resolved (DNS) o a cualquier servicio con
  socket. Para eso está `--sandbox-red`. Por eso "señales solo a sí mismo" vale para las
  llamadas de señales de node: con `--sandbox` solo, por D-Bus se le podría pedir a
  `systemd --user` que detenga o mate una unidad del usuario (no probado).
- `os.getPriority` de otro proceso sigue andando (solo lee). Un `os.setPriority` negado que el
  programa atrapa en silencio sin el sensor de violaciones (swc) no se reconoce, como un permiso
  negado (abajo).
- Con `--sandbox-red` quedan alcanzables los sockets unix creados después de lanzar ese node
  y los que tengan espacios en la ruta; el resto del disco sigue a la vista del sistema
  operativo (lo frena el modelo de permisos de node, no el namespace).
- Un permiso negado que el programa atrapa en silencio (sin imprimirlo) sin el sensor de
  violaciones (swc) o en un callback no se reconoce: A y B ven lo mismo y se comparan.
- Un programa que imprime por su cuenta uno de los textos de la marca lleva la nota de más (y el
  minificado corre una vez de más); se compara igual.
- `--allow-inspector` (las corridas con el sensor de violaciones, oxc, y las de la cobertura de
  los re-chequeos, también swc) le deja al programa abrir una sesión del inspector **en su propio
  proceso** (`new inspector.Session()`), que node avisa que "puede invalidar el modelo de
  permisos". Abrir un **puerto** no: medido con node 24.14.1, sin más que `--allow-inspector` un
  programa lo abre con `inspector.open()`, `inspector/promises.open()`, `process._debugProcess()`
  o SIGUSR1; `--disable-sigusr1` corta los dos últimos y el preload cambia `open` por una función
  que niega (`ERR_ACCESS_DENIED`) y marca el sandbox. `NODE_V8_COVERAGE` (cobertura sin
  inspector) no sirve: con `--permission` node no escribe nada (medido, con y sin
  `--allow-fs-write` sobre el directorio).
- Los comandos `repro_original:`/`repro_minificado:` de `meta.txt` no llevan las flags del
  sandbox.
- Con `--sandbox` (y `--sandbox-red`), node se lanza con `fork` + `exec` en vez de
  `posix_spawn` (el filtro se instala entre los dos): cuesta algo más de tiempo de sistema por
  ejecución de node (medido en la muestra, con otros procesos cargando la máquina: ~1,4 ms de
  sistema más en el proceso de difuzz por cada node lanzado).

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
  - en todos los anteriores salvo los nombres, además, **B no tiene que pasar por la
    violación**. B == A' también puede ser una coincidencia: si el bug deja justo el valor que
    una variable tiene cuando la rama de la violación no corre (`""`, `false`, `0`, un `+=` que
    no suma), B corre esa rama con el bug y da lo mismo que A', que no la corre. Se mira en B
    mismo: el sensor no tiene que ver en B ninguna violación (TDZ, coerción de `Symbol`,
    `extends`); para el resto, B se corre dos veces con la cobertura de bloques de V8, sin y con
    la asunción neutralizada en B (los stubs de globals, los built-ins congelados, el contenido
    fijo, más pila; el objeto de cada `with` y la clave de cada `valueOf`/`toString`/
    `[Symbol.toPrimitive]` pasan por un interruptor del preload; `"use sloppy"` contra
    `"use strict"`, del mismo largo), y las dos coberturas tienen que coincidir. Si el minifier
    aprovechó la asunción, en B ya no queda nada que dependa de ella y el camino no cambia. Si
    cambia (o no se puede ver), inconcluso: `div/`. La cobertura de V8 no separa el lado derecho
    de `&&=`/`||=`/`??=` ni los valores por defecto de parámetros y desestructuraciones (medido
    con node 24.14.1; oxc arma `t&&=e=...` de un `if (t) t = (e = ...)`): B corre con un contador
    delante de cada uno, y las cuentas entran a la comparación. Los scripts de `vm` también
    cuentan.
  - el camino no ve una dependencia **por datos**: la asunción elige un índice, una clave o un
    valor y B recorre el mismo código con y sin ella (`["", bug][+(typeof window > "u")]`). Por
    eso, en esas mismas corridas, un **sensor de uso**: B no tiene que tocar la asunción. Toca
    `global_conocido` si lee el global (`typeof window` también; con los stubs, cada uno es un
    accessor que anota), `with` si un nombre resuelve en el objeto (un Proxy), `builtins` si lee
    un built-in después de pisarlo (accessor), `coercion` si llama a un helper (el principio de
    cada uno anota; un valor que no es una función literal, como `Array.prototype.join`, va
    envuelto) y solo sloppy si una función sloppy recibe un `this` que sloppy cambia (en la
    corrida en estricto). Si lo toca, inconcluso; si no se pudo sensar (un built-in redefinido o
    borrado), también. El contenido fijo ya no fija el texto de una función nativa
    (`function max() { [native code] }`): es el mismo en A y en B.
  - para contenido y solo sloppy, que no tienen un sensor de uso completo (el texto de una
    función del programa es justo lo que lee un positivo como `String(f)`; el `arguments` atado a
    los parámetros), un `Explica` se confirma con el original **salado**: cada literal de cadena
    en posición de dato con `dz` al final, minificado igual, y el mismo re-chequeo tiene que dar
    `Explica` también ahí (con otro valor por defecto la coincidencia se rompe). Sin literales que
    salar, o si el salado no diverge, no cambia nada.

    Cuesta, solo cuando todo lo demás ya daba `Explica`: 2 ejecuciones de node (en solo sloppy,
    además, 1 o 2 `node --check`), una instrumentación del minificado con SWC y, para contenido y
    solo sloppy con literales, 1 minificación y 2 node más el re-chequeo sobre el salado.

  En oxc todos los flags de su tabla tienen re-chequeo. En swc, `proxy`, `tdz`, `callee`,
  `redefine_global`, el backlog y la asunción por ruta (`issue_2719`) clasifican directo, sin
  re-chequeo.

  **Límites conocidos** (lo que no se prueba): el re-chequeo compara salidas, el camino de B y si
  B toca la asunción; no el camino de A' contra el de B. El chequeo es de control y de uso, no de
  flujo de datos completo. Medido con oxc 0.153.0 (8 celdas): los 11 programas de la revisión de
  la noche 2, los 4 de `&&=`/`||=`/valor por defecto y los 6 de dependencia por datos de la
  revisión de 7.1 (H1, H2) van a `div/`; de 75 variantes propias (12 familias × 6 formas: tabla,
  clave, `&&=`, `||=`, valor por defecto, rama; más 3 de `with`) y 18 más (3 familias × 6: texto de función y
  `.stack` elegidos por datos sin `&&`, un valor por defecto que no es literal), lo que queda en
  `known/` son positivos (B no corre el bug: con los nombres mangleados la condición sobre el
  `.stack` ya es falsa en B). Lo que no cubre:
  - el pliegue que borra la rama (de las 99 variantes de 7.1 quedan 24 filas de 656: `global` y
    `with` × contador y objeto): oxc pliega mal la condición de adentro (`Number.isInteger(5e-324)`,
    `"abc".substring(1, undefined)`) y borra la rama entera; B ya no toca la asunción y da el valor
    por defecto, como un minificado que la aprovechó. Desde B no se distingue.
  - una dependencia por datos de una asunción sin sensor de uso completo cuyo valor por defecto
    no sale de un literal de cadena: el `arguments` atado o el texto de una función del programa
    elegidos por datos contra un valor calculado (`[].join()`); la sal no lo cambia. En las
    variantes medidas, todas las de esas familias usan literales y van a `div/`.
  - el sensor de uso de `global_conocido` ve lecturas (`typeof`, `window`, `globalThis.window`),
    no `"window" in globalThis` ni `hasOwnProperty`; el de `builtins`, lecturas después de pisar
    (si el programa redefine o borra la propiedad, inconcluso); el de coerción, las definiciones
    que ve `neutralize_coercion` (no la forma abreviada `{ valueOf }`, que ya es inconclusa); el de
    sloppy, solo el `this`.
  - la cobertura no ve lo que corre en un listener de `exit` que el programa registra (el sensor
    de uso sí: anota hasta el final), en workers ni en procesos hijos. Los scripts de `vm` sí.
  - un programa cuyo camino cambia entre dos corridas del mismo texto (reloj, `Math.random`,
    GC) da otra cobertura: va a `div/` aunque sea una asunción, igual que un B que cambia de
    camino o toca la asunción sin que se vea en la salida. En el corpus de SWC (7 celdas), la
    aceptación, el golden y los 14 corpus adversariales ningún caso cambió de destino respecto
    de 7.1 ni de fe0cd86.
  - sin el sensor entero en B (`sensor_caido`, `sensor_tope`), los re-chequeos de las
    violaciones y de `extends` quedan inconclusos (cuando A o B pasan el tope, el camino de B
    sale de otra corrida de B con el sensor y un tope mayor, que tiene que tomar el mismo camino
    que B sin el sensor: ver el tope del sensor abajo);
  - solo sloppy por el renombre de `arguments` (`Renombrando`) compara contra A, no contra A',
    y no pasa por este chequeo.

  Un programa que imprime a propósito una línea con forma de marco de V8 en la rama del contenido
  fijo también termina en `known/`. Un bug que está en el
  programa pero que B no ejecuta (oxc borró la sentencia que lanza y con eso el `catch` que lo
  tenía) no deja rastro en la salida: la divergencia es entera de la asunción y va a `known/`.
  Lo que se pierde por exigir B == A' (van a `div/`): los positivos en que B no da ni lo de A
  ni lo de A' (con `with`, terser `issue_1105/assorted_*`: oxc pasa `0/0` a `NaN`, que adentro
  del `with` resuelve a la propiedad del objeto; en sloppy, terser `dead_code_2_should_warn` y
  `dead_code_constant_boolean_should_warn_more`: funciones en bloque de Anexo B), y un `.stack`
  impreso en que el minificado cambia el nombre de un marco (el mangling de `core_defaults`) o
  la cantidad de marcos (inlining).
- **Posiciones en líneas con forma de marco de V8** (`archivo:L:C`): la comparación las borra
  solo si el archivo es de difuzz o el de un marco que el preload formateó en esa corrida (A o
  B). Una línea impresa a propósito con el archivo de un marco real (`__filename`, el mismo nombre
  que un `//# sourceURL=` del programa) pierde la posición y un bug en ella no se ve. Además, las
  pausas del sensor formatean el stack de cada excepción atrapada: el archivo de un `eval` con
  `//# sourceURL=x.js` que lanza y atrapa queda anotado aunque el programa nunca lea un stack, y
  una línea impresa con `x.js:L:C` da `iguales` aunque la posición cambie (MEDIDO, revisión de la
  integración del bloque 7).
- **El sensor de violaciones atrapadas se apaga a las 256 pausas** (`MAX_PAUSAS` en el preload):
  cada excepción atrapada lo pausa y cuesta de 0,5 ms (sin marcos) a 1,4 ms (50 marcos de
  profundidad); sin tope, 10 000 excepciones atrapadas tardan de 5 a 15 s y llegan al timeout por
  defecto (5 s). Al pausar, V8 formatea el stack del
  error (puede llamar al `toString` o a `Error.prepareStackTrace` del programa, que así puede
  notar el sensor y tomar otro camino): si A o B pasan el tope, los dos se repiten y se comparan
  sin el sensor (`sensor:` en la `clasificacion:`; `comparados_sin_sensor=` en las stats). La
  comparación y los re-chequeos son simétricos (sin el sensor); las violaciones que el sensor
  ubicó antes del tope pasan a esas corridas y solo dicen dónde neutralizar. El re-chequeo de
  violaciones decide como siempre (B == A' y B no pasa por la violación): un positivo con más de
  256 excepciones atrapadas ya no va a `div/` si su violación cae entre las primeras 256 pausas.
  El camino de B, que no es simétrico (sale de una corrida con el sensor), se mira solo cuando
  todo lo demás ya da `Explica`: B corre otra vez con el sensor y un tope de 4096 pausas, y otra
  sin el sensor, las dos con la cobertura de bloques de V8 y sobre el minificado con los contadores
  de rama (los mismos que los demás caminos por cobertura: V8 no separa el lado derecho de `&&=`
  ni un valor por defecto); cuenta solo si la corrida con el sensor es entera, da lo mismo que B y
  tiene la misma cobertura que la de sin el sensor (un programa que nota el sensor y cambia de
  camino no pasa, tampoco si el cambio está solo en el lado derecho de un `&&=`). Si no, el camino no se sabe y va a `div/`
  (inconcluso). Cuesta 2 node más, uno de ellos con el sensor y hasta 4096 pausas: con muchos
  marcos puede llegar al timeout (y entonces node se repite sin el sensor, un node más) y el
  positivo se pierde. Una violación de A atrapada después del tope no se ve: el caso va a `div/`.
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
- **Aceptación de los re-chequeos** (`tests/aceptacion/rechequeos/`): 127 casos que combinan un
  bug real de oxc con un flag de asunción, en las 4 celdas de oxc (508 comparaciones).

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
