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
   optimizador, nunca del parser ni del printer. El reverso es un punto ciego: el original que
   corre (A) no es el texto del caso sino ese programa impreso, y un parser o un printer que
   cambian la semántica le pegan igual a A y al minificado. `--crudo` lo cubre donde se usa (ver
   [El texto crudo](#el-texto-crudo---crudo)). Por defecto minifica en un proceso hijo
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
   - manda `console.error`/`warn`/`trace` a stdout (`[error] ...`, `[warn] ...`, `[trace] ...`),
     y también lo que el programa escribe directo a stderr con `process.stderr.write`,
     `fs.writeSync(2, ...)` o `process._rawDebug` (`[stderr] ...`, una vez por escritura); al
     salir, imprime los globals que el programa creó. El stderr real de node no se compara (trae
     rutas y pids); tampoco lo que llega al fd 2 por otras vías: `process.stderr.end(dato)`,
     `fs.write(2, ...)`, `fs.writeFileSync(2)` o `'/dev/stderr'`, un stream nuevo sobre el fd 2 y
     los procesos hijos que heredan el stderr. Una escritura que lanza (un `chunk` inválido) deja
     el `[stderr] ` sin dato, igual en los dos lados. Lo que llega por stderr se compara igual que
     stdout: un stack escrito a stderr con el nombre de un marco mangleado (`at miFuncion` contra
     `at e`, core_defaults) va a `div/`, como ya pasaba con `console.error(e.stack)` (la decisión
     pendiente sobre marcos renombrados; 11.1 solo agranda el canal);
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

   El original tiene que terminar solo: con 0, lanzando o con un código que elige el programa
   (`process.exit(1)`, `process.exitCode = 3`, como un runner de tests que reporta fallos). En el
   último caso se comparan el código y el stdout igual que con 0 (antes de 11.2 se descartaba
   como `exit`; `exit=` sigue en la línea de stats y queda en 0). Un original que se cuelga, muere
   por señal o no enlaza un `require`/`import` se descarta (`timeout`, `senal`, `enlace`). Los
   re-chequeos de la clasificación siguen pidiendo que sus variantes del original terminen con 0
   o lanzando: con un código propio quedan inconclusos y el caso va a `div/` con la nota. La
   pasada de `--invocar` tampoco compara un original que sale con un código propio (vale la
   pasada normal, como cuando una función invocada se cuelga).

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
| `--mem-node MiB` | tope de RSS de cada ejecución de node; al pasarlo, el caso va a `recursos/` (ver [Topes de node](#topes-de-node)) | MemTotal/2 / workers (256 MiB a 4 GiB) |
| `--max-salida MiB` | tope de la salida (stdout + stderr) que difuzz junta de cada ejecución de node; al pasarlo, el caso va a `recursos/` | 16 |
| `--sandbox` | node con su modelo de permisos (ver [Sandbox](#sandbox)) | apagado |
| `--sandbox-red` | `--sandbox` más namespaces de Linux: sin red (ver [Sandbox](#sandbox)) | apagado |
| `--crudo` | además corre el texto crudo de cada caso y lo compara con el original impreso; lo que difiere va a `codegen/` (ver [El texto crudo](#el-texto-crudo---crudo)) | apagado |

## Sandbox

Por defecto node corre **sin sandbox**: el programa del corpus puede leer y escribir lo que
pueda el usuario que corre difuzz, lanzar procesos y usar la red. Con un corpus que no es de
confianza (paquetes de npm, código real) eso es un riesgo: para ese código está
`scripts/barrido-real.sh` (ver [Corpus](#corpus)), que fija `--sandbox-red` sin opción para
apagarlo. El default sigue apagado por comparabilidad (decisión del 10-oct-2026): el sandbox
cambia el nombre de la celda (`+sandbox`, `+sandbox_red`) y la caché de A no se comparte con las
corridas sin él, así que prenderlo por defecto rompería la comparación con las corridas y los
índices anteriores.

- **`--sandbox`**: cada ejecución de node (original, minificado, confirmaciones, re-chequeos,
  `--invocar`, envoltorio de módulos, stubs) corre con el modelo de permisos de node 24:
  `--permission`, `--allow-fs-read` solo para el preload, el programa (y el envoltorio de
  módulos) y el directorio de trabajo del caso, `--allow-fs-write` solo para ese directorio y
  el canal de control de difuzz, y `--allow-inspector` (con `--disable-sigusr1`) cuando corre el
  sensor de violaciones o la cobertura de un re-chequeo (ver abajo).
  Sin `--allow-child-process`, `--allow-worker`, `--allow-addons` ni `--allow-wasi`: no hay
  procesos hijos, workers, addons nativos ni WASI (tampoco `process.binding`), y
  `--no-experimental-sqlite`: `node:sqlite` no existe (ver "Límites"). Como el modelo de
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
  **Si el permiso negado no se atrapa y mata a los dos** (8.2): lo que seguía a ese punto no
  corrió en ninguno. Si A y B dan lo mismo, el caso **no cuenta como igual**: va a
  `descartes.tsv` con causa **`sandbox_corto`** y al contador `corto=` (con 7.9 contaba como
  igual sin haber probado el resto; antes de 7.9 iba a descartes con causa `sandbox`). Si
  divergen (un bug antes de ese punto, o en lo que cada uno imprime al morir), va a `div/` como
  cualquier otro. Se mira solo el throw que terminó el programa: un permiso negado que el
  programa atrapó antes no cuenta, y un programa que toca el sandbox y después muere por otra
  cosa se compara como siempre. El throw cuenta como del sandbox si el preload ve en el objeto
  lanzado el `code` de un permiso negado (aunque el stack no lo diga, como el de un addon) o si
  su mensaje o su stack tienen uno de los textos de abajo (con `--sandbox-red`, también un error
  de red sin atrapar; `node:sqlite`). Vale en cualquier pasada; en la de `--invocar` o en el
  envoltorio de módulos descarta el caso entero, como antes de 7.9. Lo que **no** se ve: un
  permiso negado que el programa atrapa y relanza como otro error sin su mensaje (cuenta como
  igual, como con 7.9), y una función invocada por `--invocar` que lanza el permiso negado (el
  preload la atrapa y sigue con las demás: no termina el programa).
- **solo el original** (`sandbox: solo el original toco el sandbox`): con el bug, el minificado no
  llega a abrir el archivo. Es un cambio de comportamiento.

Si lo toca solo el minificado (el minifier cambió qué archivo abre), se compara sin nota. Solo va a
`descartes.tsv` con causa **`sandbox`** el caso cuyo original tocó el sandbox y cuyo minificado no
pudo correr (falla del runner al lanzarlo: no se sabe si lo toca ni hay con qué comparar); en las
variantes, eso cuenta en `original=`. La celda lleva `+sandbox` o `+sandbox_red`, la caché de A no
se comparte con las corridas sin sandbox y al terminar se imprime (y queda en `CORRIDA.txt`) la
línea `sandbox: modo=... descartados=N solo_original=M ambos=K corto=J` (`N`: casos descartados por eso;
`M` y `K`: pasadas en que solo el original, o los dos, tocaron el sandbox y se compararon, también
las de las variantes de `--mutaciones`; en el carril B, un módulo mudo puede sumar dos: la pasada
cruda y la del envoltorio de módulos; un mudo, una por cada pasada que lo tocó entre la cruda, la
observada, la de `--invocar` y la de bloques, porque cada una corre A y B de nuevo. Con
`--sandbox-red`, "tocó" incluye una salida que imprime el nombre de un error de red
(`ECONNREFUSED`, `EHOSTUNREACH`, `ENETUNREACH`, `ENOTFOUND`, `EAI_AGAIN`): es una sobre-aproximación
por texto. Por ejemplo, un mudo que guarda `require('node:constants')` en un binding (los nombres de
errno están entre sus claves) suma 1 por la pasada observada y 1 más con `--invocar`, que lo vuelve
a imprimir: así da `ambos=2` con `--invocar --stubs --sandbox-red` el caso `cjs/00490` del corpus real
(`npm:foreground-child-3.3.1.tgz:package/dist/commonjs/all-signals.js` en `fuentes.tsv`; 9.4, medido).
Esas pasadas se comparan igual, con la nota; `J`: casos y variantes descartados con causa
`sandbox_corto`, que también cuentan en `K` porque los dos tocaron el sandbox). Una variante
`sandbox_corto` tiene su fila en `descartes.tsv`, cuenta en `original=` de las mutaciones y no
cuenta como comparada.

Cómo se reconoce que una ejecución tocó el sandbox: el preload anota `ERR_ACCESS_DENIED`,
`ERR_DLOPEN_DISABLED`, el `EPERM` de `kill` y el de `os.setPriority` (`uv_os_setpriority`) en su
canal cuando ve el error (no atrapado, en `--invocar` o, con el sensor de violaciones, atrapado
por el programa); con `--sandbox`, además, el envoltorio de `process._kill` anota el `EPERM` del
`_kill` de node (que no lanza: devuelve `-1`) y el de `process._debugProcess`, aunque el programa
los atrape. Con el sandbox, `os.setPriority` y `process._debugProcess` con el pid ficticio dan
`EPERM` (el filtro niega cualquier pid que no sea el real) y marcan. difuzz busca esos textos (y
`kill EPERM`, `uv_os_setpriority returned EPERM` y `No such built-in module: node:sqlite`) en la
salida. Un `EPERM` natural se ve igual: un
`kill`/`_kill` a un proceso de otro usuario (el pid 1, por ejemplo) da `EPERM` también sin el
sandbox, y con `--sandbox` marca el caso: si el original y el minificado lo hacen, se compara con
la nota de A y B (antes de 7.9 iba a descartes y un bug real al lado no se veía; MEDIDO en la
revisión de la integración del bloque 7).

**`require` de un directorio** (8.4). Para resolver `require('..')`, `'../'`, `'.'` o `'/etc'` (un
directorio que existe) node lee el `package.json` de ese directorio: un ancestro de la carpeta
temporal de difuzz (el `TMPDIR` de la máquina, `/`) o la carpeta misma, fuera de lo permitido. El
modelo de permisos lo niega en la búsqueda, y node lanzaba `ERR_ACCESS_DENIED` donde sin el sandbox
da `MODULE_NOT_FOUND` (medido con node 24.14.1): el caso moría en el `require`, en el original y en
el minificado, sin correr el resto. Con el sandbox, el preload toma ese `package.json` negado como
uno que no existe (envuelve `Module._findPath`, que lo busca, y devuelve "no encontrado"): node arma
su `MODULE_NOT_FOUND` de siempre, que va al enlace (`descartes.tsv`, causa `enlace`) o, con
`--stubs`, al stub, igual que sin el sandbox. No se lee nada: lo negado sigue negado, sin marca de
sandbox. Vale para `require`, `require.resolve` y `createRequire` (también desde el carril B). Un
archivo de afuera que existe (`require('/etc/hostname')`, `require('../x.js')` con `x.js` afuera)
resuelve y se niega al **cargarlo**: sigue tocando el sandbox. El `import` de un directorio no
cambia: da `ERR_UNSUPPORTED_DIR_IMPORT` con y sin el sandbox. Lo que sí cambia: con el sensor de
violaciones y sin `--stubs`, un `import()` de un directorio atrapado ya no marca el sandbox (node
prueba la resolución de CommonJS para armar su mensaje, y el sensor veía ese `ERR_ACCESS_DENIED`
atrapado por node).

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
- `node:sqlite` **no pasa por el modelo de permisos** de node 24 (medido con 24.14.1: crea y
  escribe una base fuera de los directorios permitidos, sin `ERR_ACCESS_DENIED`). Por eso, con el
  sandbox va apagado con `--no-experimental-sqlite`: `require`/`import` dan
  `ERR_UNKNOWN_BUILTIN_MODULE` y la ejecución queda marcada si ese mensaje sale (sin atrapar o
  impreso); atrapado en silencio, o por `process.getBuiltinModule` (que da `undefined`), no se
  marca. Web Storage (`localStorage`) viene apagado en node 24.14.1 y difuzz no lo prende.
- **El resto de los módulos de node no se auditó** contra el modelo de permisos: que un módulo
  no esté listado acá no quiere decir que respete los permisos.
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

## El texto crudo (`--crudo`)

El backend parsea el caso e imprime el original (A) y el minificado (B) desde el mismo programa
parseado: oxc con `oxc_codegen`, swc con `paren_remover`, `resolver`, `fixer` y su `emit` (ver
`src/backend/oxc.rs` y `src/backend/swc.rs`). Un bug del parser o del printer que cambia la
semántica queda en A y en B: el oráculo, que compara A con B, no lo ve. Pasa: el parser de oxc
0.153.0 tira los paréntesis de `(h) = function(){}` y la función se llama `"h"` en vez de `""`
(en node), aun con compress y mangle apagados.

Con `--crudo`, por cada caso difuzz corre además el **texto crudo** (el `input.js` con la forma
aplicada, lo que lee el backend) con el mismo runner que A (mismo archivo, preload, sensor y
sandbox), antes de cualquier descarte de A, y lo compara con A: el estado de salida y el stdout,
con las posiciones de los marcos de stack borradas (al reimprimir cambian líneas y columnas) y los
nombres de `util.inspect` sin borrar (parsear e imprimir no tiene por qué tocarlos). Si los dos se
cuelgan e imprimieron lo mismo, es `timeout`. Si difieren, no se da por diferente sin repetirlo: el
crudo y A corren otra vez, dos veces cada uno y en fresco. Si alguno no se repite a sí mismo, o el A
fresco no repite al A del caso, es `no_determinista` (si el A del caso terminó y los frescos se
colgaron, antes se repiten con un timeout mayor, por si fue la carga; si el A del caso salió de la
caché y los frescos se repiten entre sí, la entrada es vieja: se lapida, como en la confirmación de
A contra B, y sigue el A fresco); dos timeouts cuentan como
repetidos aunque hayan impreso distinto, como en el oráculo. Si ahora coinciden, iguales (o
`timeout`, si los dos se cuelgan). Si los dos se cuelgan y cada uno imprime siempre lo mismo antes
de colgarse, pero no lo mismo que el otro, lo impreso es la diferencia y sigue (kind `Stdout`); si
lo impreso cambia entre corridas (un bucle que imprime), `timeout`. Un lado en timeout se repite
con un timeout mayor, como B en la confirmación, y si sigue colgado (sea el crudo o A) va a
`codegen/` con kind `Timeout`; si alguno no corrió entero con el sensor de violaciones, los dos sin
él (cada uno con el timeout con el que terminó). Si el crudo leyó `Function.prototype.toString` o
`.stack`, se aplica el mismo paso `contenido` que a A contra B (`clasificar::paso_contenido`, con
el crudo como original y A como minificado): es `contenido` (la asunción de `toString` de oxc, no
un hallazgo) solo si el crudo con otro contenido (salado) da otra salida, con el contenido fijo del
preload el crudo y A coinciden, A se comporta como el crudo fijo (o difiere solo en líneas que
muestran ese contenido: un `toString` impreso, un marco salvo la posición), la cobertura de A no
cambia con el contenido fijo y, si el crudo tiene literales en posición de dato, el mismo paso
sobre el crudo con esos literales salados también lo explica (sin literales que salar no hay
confirmación con la sal y alcanza con lo anterior, como en A contra B). Si no, o si no se puede
saber, sigue a `codegen/` con el paso en `pasos:`. Así, un bug de codegen en una rama que depende del
texto de `toString` o de `.stack`, o que solo se ve en un marco del stack, ya no se excusa (10.1b,
MEDIDO con O8 de oxc 0.153.0: los tres repros de las revisiones de 10.1 van a `codegen/`). El
precio, el mismo que en A contra B: un valor derivado del contenido (`String(f).length`,
`stack.length`) da inconcluso y va a `codegen/` aunque solo difiera el texto.

Lo confirmado va a `codegen/<caso>/`: `crudo.cjs` y `original.cjs` (`.mjs` en el carril B), lo que
imprimió cada uno (`crudo.stdout`, `a.stdout`, ...) y un `meta.txt` con el `kind` (el crudo hace de
original y A de minificado: `ThrowNew` es que A lanza y el crudo no), los pasos de la confirmación
y dos comandos para reproducir cada lado. **No va a `div/`**: no es un bug del minificador (B lo
hereda de A) sino del parser, del printer o de la cadena del backend antes del minificador, y se
reporta a otro componente. Es **ortogonal**, como `lint/`: el caso sigue a la comparación de A con
B como siempre (y termina en `div/`, `known/`, iguales o descartes), así que un bug del minificador
en el mismo caso nunca se pierde por el chequeo, y la identidad de `total=` no cambia. Con la
opción, una línea más en las stats (y en `CORRIDA.txt`):

```
crudo (10.1): comparados=N identicos=N iguales=N CODEGEN=N contenido=N no_determinista=N timeout=N runner=N | cache_hits=N
```

`comparados = identicos + iguales + CODEGEN + contenido + no_determinista + timeout + runner`
(`identicos`: el texto crudo ya es el impreso; `timeout`: los dos sin terminar y nada estable que
comparar; `runner`: error de difuzz con el crudo, también un panic de difuzz en el chequeo, que no
corta la comparación de A con B del caso). El chequeo no toca el estado del caso: si su sal hace
saltar un `debug_assert` del backend o falla un análisis de difuzz, sale por stderr con el caso y no
llega a `asserts/`, al `meta.txt` de A contra B ni a `fallos_analisis` (10.1c). El crudo se guarda
en la caché de A (no depende del preset: lo comparten las celdas del mismo carril); `cache_hits` cuenta esos aciertos. Con `--indice`, cada `codegen/` es una
fila con destino `codegen`, y `--cruce` agrega `codegen=N` a los totales por celda (solo si alguna
celda dejó uno). La celda no cambia de nombre.

**Costo y ruido** (MEDIDO, 12-oct, fixtures de SWC, 3119 casos, oxc carril_a, `--no-cache`,
`--jobs 8`): una ejecución de node más por caso (y cuatro más, solo en los que difieren: el crudo
y A dos veces cada uno). A eso se suman, solo donde aplica (`main::comparar_crudo`, 10.10): dos
más de A con el timeout largo si terminó en el caso y las dos frescas se colgaron; una del lado
colgado con el timeout largo si uno solo se cuelga; las dos sin el sensor si alguno corrió sin el
sensor entero; y, si el crudo tiene `fn_tostring` o `stack`, el paso `contenido` de A contra B
(`clasificar::paso_contenido`): el crudo salado, el crudo y A con el contenido fijo, hasta dos de A
con cobertura y, si da `Explica`, la sal de literales (una reimpresión con SWC, una minificación,
dos node y el mismo paso sobre lo salado). La
corrida pasa de 43,0 s a 68,3 s en el carril A (+59 %) y de 51,8 s a 79,3 s en el B (+53 %); con la
caché, el crudo se ahorra en las celdas siguientes del mismo carril. La línea `total=` y los
destinos son los mismos con y sin la opción. De 2754 casos comparados en el A (3032 en el B):
`CODEGEN=0`, `contenido=2` (`terser/compress/evaluate/issue_2919` y `prop_function`, que imprimen
`Function.prototype.toString`), `timeout=6` (los mismos 6 que A no termina) y `no_determinista=0`.
Con el criterio de `contenido` de 10.1b, las mismas cifras en los dos carriles (MEDIDO, 12-oct,
mismo corpus y opciones): los dos `contenido` imprimen el `toString` entero. El stack impreso
en stdout de `issue_2208_8` da iguales (las posiciones se borran) y el `Math.random` de
`fixture/issues/8119` también (el preload lo fija). Los casos `contenido` y
`no_determinista` no tienen directorio: salen por stderr (`crudo: ...`). En `tests/codegen/` (142
casos) el costo es despreciable.

**Qué cubre y qué no.** El punto ciego queda cubierto solo donde se usa la opción y solo para lo
que el corpus ejercita. `--crudo` corrió sobre los fixtures de SWC (oxc, carriles A y B: 0
`CODEGEN`, ver arriba) y no sobre el corpus real: el 0 de ese corpus es del arnés aparte del
agente codegen-fidelidad (24 933 ejecuciones de los fixtures de SWC y del corpus real, 11-oct;
ver [`tests/codegen/README.md`](tests/codegen/README.md)), no de esta opción. El bug de los
paréntesis salió de formas escritas a propósito, que están en `tests/codegen/` (ver
[Tests y verificaciones](#tests-y-verificaciones)). No mira la pasada observada (el epílogo de un
programa mudo), el envoltorio de módulos del carril B, `--invocar` ni las variantes de
`--mutaciones`: un mudo se compara solo por su estado de salida y sus globals. Se compara lo que
el programa imprime en stdout, con los mismos límites que el oráculo: `console.error`,
`console.warn` y `console.trace` sí cuentan (el preload los pasa a stdout como `[error] ...`,
`[warn] ...`, `[trace] ...`), y también lo escrito directo a stderr por las vías que cubre el
preload (`[stderr] ...`, ver [Cómo funciona](#cómo-funciona)), pero el stderr de node no, y de un `throw` o un rechazo no
atrapados solo se compara el tipo (`!dz throw <Tipo>`), no el mensaje: un crudo y un A que
lanzan el mismo tipo con otro mensaje dan iguales. Un bug del parser que hace que el texto no
parsee (`descarte:parse_*`) no llega al chequeo. Si el crudo y A se cuelgan los dos e imprimen lo
mismo (o algo que cambia entre corridas), no hay nada que comparar: cuentan en `timeout=`, sin
directorio y con una línea por stderr (`crudo: los dos se cuelgan: <caso>`); es lo mismo que A
contra B, que descarta un original que no termina, y un bug de codegen que no cambia lo impreso
antes del cuelgue no se ve. Un timeout por carga en una sola de las corridas frescas (un lado
que se cuelga en una y en la otra no, o un A del caso que no salió de la caché, se colgó y en
fresco termina) da `no_determinista`, también con su línea por stderr (`crudo: no determinista
...`), aunque la diferencia con el crudo fuera confirmable.

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
    ├── recursos/<caso>/       el minifier no terminó en 60 s o pasó el presupuesto de memoria, o
    │                          una ejecución de node pasó un tope de difuzz (ver Topes de node)
    ├── lint/<caso>/           sospechosos del lint: el minificado escribe sobre literales nuevos
    └── codegen/<caso>/        con --crudo: el texto crudo y el original impreso dan otra salida
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

  `lint/`, `asserts/` y `codegen/` son ortogonales: el caso cuenta también donde terminó.
- **`--indice DIR` y `--cruce DIR`.** Con `--indice`, cada celda deja `DIR/index-<celda>.tsv`
  al terminar (una corrida interrumpida deja intacto el índice anterior). `difuzz --cruce DIR` lee esos índices y, sin correr
  nada, imprime una tabla caso × celda con el `kind` de cada hallazgo (`S`=Stdout,
  `N`=ThrowNew, `P`=panic, ...) y los totales por celda. Solo una corrida entera publica su
  índice (10.2b): si terminó con escrituras perdidas (también del índice mismo, de
  `known-sintomas.tsv` o de `lint-triage.tsv`) o con casos sin contar, o si el índice no se pudo
  crear, el índice anterior queda como vigente, la corrida sale con 1 y `incompleta`, y
  `--cruce` lo avisa al final (`<celda>: incompleta (...)`, marca `index-<celda>.tsv.parcial`).
  Si el directorio del índice no se puede escribir ni siquiera para la marca, `--cruce` muestra
  el índice anterior sin aviso: la corrida lo dice por stderr y en `CORRIDA.txt`
  (`indice_no_creado:`). `--cruce` también avisa las líneas ilegibles de un índice (una
  escritura cortada).

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
  - los nombres (minificar preservándolos, solo en core_defaults y si la salida puede mostrar
    un nombre: el original lee `.name` o A imprime una instancia `X {`), en los dos: que la
    divergencia desaparezca y (11.3) que B difiera de A solo en nombres: mismo estado y, en cada
    línea distinta, lo que cambia (salvo espacios) son identificadores del código de ese lado (el
    original para A, el minificado para B; los saca el parser de SWC, no las palabras de un
    string) que no son un valor (`NaN`, `undefined`, `true`, lo que da `typeof`, ...). Con otra
    cantidad de líneas (`util.inspect` corta una instancia según el largo del texto, y el nombre
    largo en A y mangleado o ausente en B cambia el corte) se compara la salida entera sin blancos,
    token por token en la misma posición, sin los nombres que preceden a un `{` (el prefijo de una
    instancia): lo distinto tiene que ser nombres, y un token de más (una línea repetida) o un
    cambio solo de saltos de línea no pasa. Si no, en oxc, alcanza con que el minificado sin mangle dé
    lo mismo que A y B sea un renombre de él (la misma prueba de `mensaje_nombre`, abajo): así
    pasa un valor que sale de un nombre (`inferred.name === 'inferred' ? f : null`, un paquete
    del corpus real). keep_names también apaga pasadas de oxc (el inlining de una función
    anónima de un solo uso, su borrado, el del nombre de una expresión de función): un bug en una
    de ellas desaparece con keep_names igual, y antes iba a `known/` con que el programa nombrara
    `.name` en cualquier lado. Si la divergencia desaparece pero no pasa ninguna de las dos:
    `Inconcluso`, `div/` con nota y `nombres_inconcluso`. Costo medido: `fixture/issues/12306` de
    SWC (`f.name === 'g'` da `true` contra `false` porque oxc borra el nombre de la expresión al
    inlinearla) va de `known/` a `div/` en las dos celdas core_defaults de oxc: por la salida no
    se distingue de un bug del inlining que cambia un booleano. No hay paso de camino (abajo): B
    y el minificado con keep_names son textos distintos y sus coberturas no se comparan. Límite:
    un bug que solo cambia qué nombre se imprime (B llama a otra función e imprime su `.name`
    mangleado) y que keep_names apaga pasa por un nombre.
  - el **nombre local en un mensaje de V8** (`mensaje_nombre`, solo oxc; no es una asunción de oxc
    sino una decisión de difuzz, como `stack`): V8 arma `X is not a function` (también `is not a
    constructor`, `is not iterable` y sus variantes async) con el texto de la expresión, y el
    mangle lo cambia (`cb is not a function` en A, `e is not a function` en B). El paso corre
    solo si A y B tienen las mismas líneas y cada una que difiere es ese mensaje con otra
    expresión de nombres (`cb`, `o.cb`, `this[#h]`, `a[0]`, y como escribe V8 las llamadas y los
    accesos opcionales: `f(...)`, `f(...)(...)`, `o.f(...).g`, `o?.g`, siempre `(...)` sin los
    argumentos, medido en node 24); si no, ni corre ni deja traza. Se
    minifica de nuevo con el mismo preset y **mangle apagado** (B0), y `Explica` pide: (1) que B
    sea **alfa-equivalente** a B0, comprobado con el resolver de SWC (independiente del de oxc):
    los mismos nodos salvo los nombres, cada identificador de B resuelve al binding que
    corresponde al de B0, los globales y los nombres de import/export no cambian, y los
    privados se corresponden por clase; (2) que B0 dé lo mismo que A, y (3) que B difiera de B0
    solo en esos mensajes. Con un preset que promete keep_names (carril_a; sale de las opciones
    que arma el backend, `Minifier::preserva_nombres`), además, el renombre no puede tocar un
    binding del que sale el `.name` de una función o clase: su nombre propio (declaración o
    expresión con nombre, método privado) o el destino de NamedEvaluation de una definición
    anónima (`var x = () => {}`, un default de parámetro o de destructuring, `x = ...`,
    `x ||= ...`, `x &&= ...`, `x ??= ...`, un campo privado; `recheck::Nombran`; con el destino
    entre paréntesis, `(x) = ...` o `[(x) = ...] = []`, no hay NamedEvaluation y no se marca:
    node da `.name` vacío, y el codegen de oxc 0.153.0 quita esos paréntesis igual). Si lo toca, el
    `.name` cambió, y eso ya es un bug de keep_names que el programa puede estar imprimiendo con
    la forma del mensaje (oxc 0.153.0 manglea un parámetro con default anónimo:
    `(handler = () => {}) => handler.name` da `e`): `Inconcluso`, `div/` con nota (H1 de la
    revisión del bloque 8). Se mira el binding renombrado, no el texto: no importa cómo lea el
    programa el `.name` (`f["na" + "me"]`, `Reflect.get`). Sin keep_names (core_defaults) el
    cambio del `.name` es la asunción "No Reliance on Function.prototype.name" y no se mira.
    Lo que keep_names promete es `.name`, no el nombre con que V8 rotula un marco de stack: con
    `const h = c ? function () {} : 0`, `.name` es `""` pero el marco dice `at h` (V8 lo infiere
    del binding fuera de NamedEvaluation; medido en node 24 por la revisión del bloque 8). Ese
    nombre no se marca: se ve solo leyendo `.stack`, que ya prende el flag `stack` (asunción
    aceptada).
    Un mangle que hace que la llamada capture otro binding no llamable
    (mismo mensaje con otro nombre) no pasa (1): `Inconcluso`, `div/` con nota. También es
    inconcluso si hay `with` o `eval` directo (la resolución es dinámica; cuenta cualquier llamada
    a un nombre `eval`, con paréntesis o no, también un binding local llamado así, que puede
    valer el `eval` de verdad: `(eval)(x)` es directo, `(0, eval)(x)` y `eval?.(x)` no) o una
    forma que la comparación no reconoce. No entran: un marco de stack con otro nombre de función
    (`at run` → `at e` en core_defaults: sigue en `div/`), `X is not defined` (un global no se
    manglea) y las expresiones que V8 escribe con algo más que nombres, puntos, corchetes,
    llamadas y opcionales (`(intermediate value)`, `(0 , x.f)`, un literal). Cuesta 1
    minificación (en un hijo nuevo, con el timeout y el presupuesto de memoria de los otros
    minificadores del worker; si no termina, `Inconcluso` con la causa), 2 parseos de SWC y 1
    node, solo cuando la divergencia pasa ese filtro.
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
    delante de cada uno, y las cuentas entran a la comparación. Los contadores (y el sensor del
    `this` de sloppy) se insertan en el texto de B, sin re-emitirlo, y el `toString` de una función
    devuelve su texto sin lo insertado: un programa que lee el texto de sus funciones ve el de B
    (`String(f)` de una función con un valor por defecto). Los scripts de `vm` también cuentan.
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
  - para contenido, solo sloppy y profundidad, que no tienen un sensor de uso completo (el texto de una
    función del programa es justo lo que lee un positivo como `String(f)`; el `arguments` atado a
    los parámetros; profundidad no tiene ninguno, y un desborde de pila nativo usado como dato no
    cambia el camino de B: ver Límites conocidos), un `Explica` se confirma con el original **salado**: cada literal de cadena
    en posición de dato con `dz` al final, minificado igual, y el mismo re-chequeo tiene que dar
    `Explica` también ahí (con otro valor por defecto la coincidencia se rompe). La sal toca solo
    literales de cadena (y plantillas sin expresiones) que sean elemento de un arreglo, valor de
    una propiedad, inicializador de una declaración, lado derecho de una asignación u objeto de un
    acceso (`"abc".substring`); no toca los demás (argumentos, comparaciones, claves, `return`,
    ramas de `?:` o de `||`) ni números, booleanos o `null` (ver Límites conocidos). Sin literales que
    salar, contenido y solo sloppy no cambian nada. Si el original salado ya no diverge, la sal cambió el camino (un
    literal que guarda la rama del bug, `modo === "x" ? bug : ...`): inconcluso, porque el salado
    no confirma nada. Profundidad (9.2) es más estricta: sala además los argumentos de una llamada o
    de `new` (no el especificador de `require` ni de `import()`), las ramas de `?:`, el lado
    derecho de `||`, `&&` y `??`, el de `return`, el cuerpo expresión de una flecha y los valores
    por defecto; sin literales que salar es inconcluso; y la sal tiene que **llegar** a lo que B
    imprime distinto de A: cada línea de B que difiere de la de A tiene que traer, en B salado,
    más `dz` que en B (sin distinguir mayúsculas), con las mismas líneas; y (10.2) B salado tiene
    que ser esa línea de B con `dz` insertados, y al menos uno tiene que caer dentro o al final de
    un token de B que difiere de A (tokens: corridas de letras, corridas de dígitos y cada signo
    suelto), no después de una etiqueta que A también imprime. "Difiere" es con la
    comparación de la celda: con keep_names (carril_a, 9.1) una línea que difiere solo en un
    nombre que imprime `util.inspect` también cuenta, y como la sal no llega a un nombre, es
    inconclusa (un positivo que imprime una función elegida por el desborde va a `div/` en
    carril_a). Si la sal toca otra
    cosa (una etiqueta, otra línea, un literal que el programa convierte en número), que el
    salado siga dando `Explica` no prueba que la coincidencia de B con A con más pila venga de
    un dato: inconcluso. La traza de meta.txt dice cómo terminó la sal en los tres: `Explica (...;
    confirmado con los literales salados)` o `Explica (...; sin literales que salar: no se
    confirmo con la sal)` (en profundidad, esto último ya no pasa: es inconcluso, con su nota). La confirmación salada de profundidad (8.1) no depende de la política del
    backend: vale en las celdas de swc también, y un `Explica` de profundidad en swc paga el
    salado y puede ir a `div/` como en oxc (en el corpus de tests de SWC ninguna divergencia de
    swc llega ahí: sin cambios en `swc a`, `swc b` ni `swc a --sandbox`, medido en la revisión del
    bloque 8).

    Cuesta, solo cuando todo lo demás ya daba `Explica`: 2 ejecuciones de node (en solo sloppy,
    además, 1 o 2 `node --check`), una instrumentación del minificado con SWC y, para contenido,
    solo sloppy y profundidad con literales, 1 minificación y 2 node más el re-chequeo sobre el
    salado (en profundidad, hasta 5 node más: A salado con más pila, A salado otra vez, B salado
    con más pila y las 2 corridas de B salado con cobertura, más una instrumentación del
    minificado salado). 9.2: si la sal no llega a las líneas de B, esas 5 no corren; sin
    literales que salar, ninguna. Lo que la sal amplia agrega es que un `Explica` de profundidad
    con los literales solo en argumentos o en ramas de `?:` ahora paga el salado (MEDIDO con
    `strace`, a/core_defaults: 12 → 19 node).

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
    la sal no cambia: un valor calculado (`[].join()`), un literal fuera de las posiciones que se
    salan (`elegir("", bug, cond)`) o un número, booleano o `null` (`[0, bug.length]`); la asunción
    es el `arguments` atado o el texto de una función del programa, elegidos por datos. Medido con
    oxc 0.153.0 en las 8 celdas de la revisión de 7.1b, con B ejecutando el bug, en `known/`: el
    `arguments` atado contra `[].join()` (4 filas: las de carril A), el texto de una función (8) y,
    de sloppy, una función declarada en un bloque (Anexo B: `typeof fb` fuera del bloque, 4 filas;
    otra diferencia de sloppy sin sensor, además del `this`). En la revisión final del bloque 7
    (oxc 0.153.0, 8 celdas, B ejecutando el bug, igual que `fe0cd86`), también en `known/`: el
    texto de una función con `""` como argumento (8 filas), el `arguments` atado con `""` como
    argumento (4) y el texto de una función con valor por defecto `0` (8). Con un literal de cadena
    en posición de dato (`["", bug][i]`) van a `div/`.
  - la profundidad, cuando la salida depende como dato de un desborde de pila **nativo** (por
    ejemplo, si `JSON.stringify` de un arreglo anidado 7000 veces lanzó o no) y el bug deja en B
    el valor de A con más pila pegado a un literal salado que el programa imprime en el **mismo
    token** (letras con letras: `"es" + bool` imprime `estrue`; dígitos con dígitos: `"1" + t[i]`
    imprime `10` contra `12`; la sal cae adentro del token que difiere; MEDIDO el de dígitos en
    la revisión A del bloque 10, x01, `known/` en las 4 celdas de oxc). Hasta 10.2 bastaba con la
    misma línea, antes o después del valor
    (`console.log("valor:", [0, bug.length][i])`, `console.log([true, bug][i], "es entero")`,
    `"n=".concat(...)`): la sal llegaba a la línea por la etiqueta, `sal_alcanza` la daba por
    alcanzada y el caso iba a `known/` (MEDIDO en la revisión del bloque 9, rev-b9-A s01 y s02, y
    con `c9eab79` en los `26-b102-n`); desde 10.2 la sal tiene que caer en un token que difiere de
    A y van a `div/`. El costo, del lado seguro: un positivo cuyo valor elegido es una cadena vacía
    (`''` contra `'bc'`: la sal cae después de la comilla, que A también imprime) o cuya línea la
    sal cambia en algo más que agregar `dz` (una cuenta, un largo) es inconcluso y va a `div/`. Una
    línea que no se alinea dentro del tope (4 millones de celdas) también. El re-chequeo de profundidad no tiene sensor de uso, y con una recursión
    nativa B recorre los mismos bloques con y sin más pila, así que su camino no cambia; desde 8.1
    su `Explica` se confirma salado y desde 9.2 la sal tiene que llegar a la línea (ver arriba).
    Medido con oxc 0.153.0 en las 4 celdas de oxc: con `9e52cef` iban a `known/` la tabla con los
    valores en argumentos (`[String(""), String("abc").substring(1, undefined)]`, sin nada que
    salar), la misma con un literal salable que no es el de la coincidencia, un booleano
    (`[true, Number.isInteger(5e-324)]`), un número (`[0, bug.length]`, el residuo de 8.1), la
    etiqueta en otra línea y la tabla en las ramas de un `?:`; con 9.2 van a `div/`. Las formas de
    la etiqueta en la misma línea, antes o después del valor o pegada a él en otro token (`n=0`),
    con 10.2; pegada en el mismo token, no (arriba). El costo: un positivo de verdad (el original
    desborda y el minificado no) que imprime un número o un booleano (`console.log(rec(4800))`)
    no tiene nada que salar y va a `div/` como `profundidad_inconcluso`; con la tabla en un
    arreglo, en las ramas de un `?:`, en un `return`, en un argumento, en una plantilla o pasada
    por `toUpperCase`, sigue en `known/`. Con una recursión de JS la cobertura cambia y va a `div/`.
  - la profundidad nativa (como arriba) cuando el valor del bug no se imprime: **elige** qué
    literal se imprime, y A con más pila elige el mismo. `console.log(t[i] === 0 ? "cero" :
    "otro")` con `t = [0, bug.length]` (A: `otro`; B y A con más pila: `cero`), o el mismo valor en
    un `if` que imprime una línea que A no tiene (`if (t[i] === 0) console.log("cero")`). La sal
    llega a `cero`, el token que difiere, y el salado también coincide: `known/` (MEDIDO en la
    revisión A del bloque 10 con oxc 0.153.0 y el bug F, `rev-b10-A/corpus-sal` x02, x03 y x05,
    en las 4 celdas de oxc; igual con `c9eab79`). Por la salida no se distingue de un positivo de
    verdad (`rec(4800) ? "desbordo" : "cupo"`, que sigue en `known/`): la sal no lo puede cerrar.
    Caso fijo `26-b102b-l01` en la aceptación, con su destino actual.
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
  - el programa puede reconocer la instrumentación o interferir con el preload. El canal y el
    cálculo de la cobertura usan lo que el preload toma antes de que corra el programa
    (`fs.writeSync`, los métodos de String, `Math.imul`, `Object.keys`, `Session.prototype.post`) y
    recorren con índices; los ganchos `__difuzzNeutro*` solo existen cuando el texto que corre está
    instrumentado. Pero un programa que encuentra los ganchos o los contadores (por ejemplo,
    listando los globals en la corrida instrumentada), que cuenta las pausas del sensor en su
    `Error.prepareStackTrace` o que escribe en el canal de control puede elegir otro camino solo
    en las corridas de difuzz, y un bug real puede terminar en `known/`. Es la misma clase que un
    programa que detecta el sensor de violaciones. Lo que se cerró (medido): pisar al final
    `String.prototype.startsWith`, el iterador de los arreglos o `Session.prototype.post` dejaba
    la cobertura vacía e igual en las dos corridas, y eso mandaba el bug a `known/`; ahora va a
    `div/`.
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
- **Los nombres de funciones y clases que imprime `util.inspect`** (`[Function: f]`,
  `[class A]`, `bound f`, `#f`, ...) **se comparan según la celda** (9.1). Sin keep_names
  (core_defaults) la comparación los borra (`oracle::normalize_names`): el mangle y los nombres
  inferidos los cambian sin que sea un bug. Con keep_names (carril_a, `Minifier::preserva_nombres`)
  el minifier promete el `.name`, y todas las comparaciones del caso los ven crudos: la de A con B,
  las de los re-chequeos (B == A', las líneas de `localizada_con_marca`, `mensaje_nombre`) y el
  síntoma de `known-sintomas.tsv` (por eso su encabezado pasó a `sintoma v3`: las tablas viejas se
  descartan una vez). `console.log(handler)` con `handler` un parámetro con default anónimo que oxc
  0.153.0 manglea da `[Function: handler]` contra `[Function: e]` y va a `div/` con el paso
  `nombres_inspect` en la traza (antes de 9.1 iba a `iguales` en las 4 celdas: H3 de la revisión
  del bloque 8; casos `25-b91-`). El modo lo fija `main::process_case` por caso
  (`oracle::con_nombres_crudos`, por hilo). Lo que cuesta, MEDIDO en el corpus de SWC (3119 casos,
  9e52cef contra 9.1): en oxc carril_a y core_defaults ningún caso cambia de destino; en swc
  carril_a, 3: `hoist_props/issue_2508_5` y `_6` pasan de `iguales` a `div/` (`o.f = function
  (){}` movida a una variable mangleada: `[Function: f]` contra `[Function: n]`) y
  `reduce_vars/unsafe_evaluate_object_2` a `known/` (backlog de SWC; `[Function: cube]` contra una
  anónima). Son los casos por los que se agregó `normalize_names` en septiembre: el `.name`
  cambia aunque swc tenga `keep_fnames`; si eso es un bug de swc o algo que `keep_fnames` no
  promete no está decidido, y quedan en `div/` (la dirección segura). Sigue borrándolos A contra
  A (`main::same_exec`: el determinismo, la caché envenenada y el renombre de `arguments` del
  re-chequeo sloppy): ahí los nombres no salen del minifier. B contra B con contadores o con el
  sensor (`recheck::comparar_caminos`, `camino_sensor_i3`) pasa por `compare` y también los ve
  crudos: si el texto instrumentado cambiara un nombre inferido, el camino queda sin probar y el
  caso va a `div/` (la dirección segura; no medido aparte).
- **La pasada observada y `--invocar` ven el `.name` de una función desde 10.3**: lo que
  exporta, observa o devuelve el programa pasa por `ser` del preload (`src/preload.js`,
  `serFuncion`), que hasta 10.3 escribía toda función como `fn`, sin nombre: un bug de keep_names
  que solo se veía en lo que exporta un módulo mudo quedaba en `iguales` también en carril_a
  (MEDIDO en la revisión del bloque 9, rev-b9-A `corpus-obs`: `export function make(handler = ()
  => {}) { return handler; }` con `--invocar`, o01, y un objeto exportado con esa función, o02).
  Ahora la escribe como `util.inspect`, `[Function: handler]` o `[Function (anonymous)]`, y la
  comparación la trata como al resto de los nombres (arriba): borrada en core_defaults, cruda en
  carril_a (o02 va a `div/` en oxc carril B carril_a, caso `26-b103-o02`; o01 con `--invocar`, el
  test `e2e_tests::nombre_de_funcion_invocada_de_punta_a_punta`). Sin getters ni traps: un
  `name` que no es un dato string (un `static get name()`, un Proxy) sale `[Function]`. Una clase
  sale también `[Function: A]` (no `[class A]`: el tipo saldría del código fuente). El constructor
  de una instancia serializada no se escribe (`inst{...}`): un nombre de clase solo visible así
  sigue sin verse. El preload es parte de la clave de la caché de A (`main.rs`, `cache_parts`):
  las entradas viejas no se reusan. Ruido conocido en swc carril_a (H2 de la revisión A del
  bloque 10, MEDIDO en una muestra de 300 fixtures de SWC: `swc a` y `swc b` pasan de 1 a 4
  `div/`; en oxc, 0): SWC inlina un binding de un solo uso (`var a1 = () => ...`, `var nameless =
  class {}`) dentro de la llamada de observación que agrega difuzz (`__difuzzObserve(["a1",
  () => ...])`) y la función pierde el nombre inferido (`[Function: a1]` contra `[Function
  (anonymous)]`). El único uso es el epílogo de difuzz y el programa nunca lee `.name`; si
  `keep_fnames` de SWC promete los nombres inferidos no está decidido (como los casos de 9.1): va a
  `div/`, la dirección segura. En el corpus completo no se midió.
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
  Lo mismo cuando B corrió con el sensor **perezoso** (8.7): si el minificado no tiene a la vista
  nada que pueda atrapar (`runner::puede_atrapar`), B corre sin prender el sensor y no deja líneas
  `viol`, y un `try` armado en runtime que atrapa la violación no se veía: un bug en ese `catch`
  iba a `known/`. Ahora, si todo lo demás ya da `Explica`, el camino de ese B se busca como acá
  (B con el sensor entero, sin el pre-filtro, y B sin el sensor, con la misma cobertura): los
  mismos 2 node más. La corrida con el sensor de este camino, también la de I3, va siempre con el
  sensor entero (`Runner::sensor_entero`).
- **Un minificado que node no acepta nunca se explica con una asunción**: ninguna permite
  producir código inválido.
- **Una conocida que cambia de síntoma vuelve a `div/`** (se compara contra
  `known-sintomas.tsv`).

La sección `clasificacion:` de `meta.txt` muestra cada re-chequeo y su resultado.

**Sin el sensor de violaciones.** En oxc, el preload ve las violaciones atrapadas (TDZ, coerción
de `Symbol`, `extends` que lanza) con el inspector de V8, que pausa en cada excepción atrapada. A
y B se comparan los dos sin ese sensor (paso `sensor:` de `clasificacion:`) en dos casos:

- node muere o no termina con el sensor: se repite sin él (`sensor_caido`), con el timeout
  entero. Límite conocido (decisión del 10-oct-2026): un programa que cuelga de verdad paga dos
  timeouts enteros en cada ejecución de node que corre con el sensor (A, B, la confirmación y
  cada re-chequeo), porque antes no se puede saber si la demora es del sensor. Un timeout más
  corto para la repetición (un cuarto, mínimo 1 s) perdía un `div/` real: una recursión que
  atrapa, lenta con el sensor, seguida de un bucle largo y del bug;
- de entrada, si el original del carril A es sloppy (sin un `"use strict"` en el prólogo), tiene
  una llamada directa a `eval` y alguna clase (`sensor_omitido`; resumen:
  `sensor (b7-8): omitido_por_riesgo_v8=N`). Es el patrón de un bug de V8 13.6 (node 24.14.1):
  una pausa dentro de la definición de una clase en ese código arma los scopes con el contexto
  equivocado, y node muere con SIGSEGV en el GC o sigue leyendo memoria fuera del contexto
  (medido con un repro de tres líneas, sin difuzz).

En esos casos no hay violaciones atrapadas: una divergencia que solo ellas explicarían queda en
`div/`, nunca en `known/`. En el corpus de tests de SWC son 8 de 3119 `input.js`. El patrón no ve
una clase o un `eval` que solo están en strings (código armado en runtime): para eso queda el
reintento.

## Topes de node

Cada ejecución de node (el original, el minificado, las confirmaciones, los re-chequeos, las
pasadas observadas, `--invocar`, las variantes y el crudo) corre bajo topes de difuzz. Lo que pasa
un tope no es un resultado del programa: lo que imprimió no se compara con nada. El caso (o la
variante) va a `recursos/` con la causa en `meta.txt`, el mensaje y el programa que corrió
(`programa.cjs` o `.mjs`: el original, el minificado o el de un re-chequeo), nunca a iguales, a
`div/`, a `known/` ni a `descartes.tsv`. En la línea de stats: `RECURSOS=N (timeout=… memoria=…
node_memoria=… node_salida=…)` (las dos primeras son del minificador; las variantes de
`--mutaciones` suman en el `RECURSOS=` de su línea). Si lo pasa una ejecución del chequeo de
`--crudo`, el chequeo cuenta como `runner=` en su línea y el caso sigue. Al arrancar, difuzz
imprime los dos topes (`topes de node: ...`).

- **Memoria (`--mem-node MiB`, causa `node_memoria`).** difuzz lee el RSS de node cada ~10 ms
  mientras corre y, al cosecharlo, su pico (`ru_maxrss`): si pasa el tope, lo mata (a él y a su
  grupo). El default es la misma cuenta que el presupuesto de los hijos minificadores (MemTotal/2
  repartido entre los workers, entre 256 MiB y 4 GiB; `--mem-hijo` no lo cambia): cada worker
  hace una cosa a la vez, así que `--jobs` × tope acota lo que corre junto y la otra mitad queda
  para difuzz y el resto de la máquina. Con 32 workers y 31 GiB son 496 MiB; con un solo caso,
  un solo worker y 4 GiB. Un caso legítimo que necesita más va a `recursos/`: se repite con
  `--mem-node` más alto o menos `--jobs`. Lo que un programa reserva y toca entre dos lecturas
  pasa el tope antes de que lo maten (medido: 267 MiB con un tope de 256, reservando de a
  32 MiB). Mide solo a node, no a los procesos que el programa lance (sin `--sandbox`). No se usa
  `RLIMIT_AS`: V8 reserva 1,4 GB de espacio virtual para arrancar y WebAssembly ~10 GB, y con
  el límite una reserva grande es un `RangeError` que el programa ve (y A y B darían iguales con
  otro programa); `--max-old-space-size` solo cubre el heap de V8 (no los `Buffer`).
- **Salida (`--max-salida MiB`, causa `node_salida`; default 16 MiB).** Lo que difuzz junta de
  stdout y stderr, sumados. Lo que pasa el tope ya no se guarda (se sigue leyendo y se tira) y node
  muere: una salida cortada no se compara como si estuviera entera. Antes la salida se juntaba
  entera en la memoria de difuzz, unas 5 veces (512 MiB de stdout: 2,6 GB de RSS). La salida más
  grande de las cachés de A de las corridas del 7 al 12 de octubre (254 442 ejecuciones, con el
  corpus real) pesa 1,4 MiB con el programa incluido. Un minificado que escribe de más donde el
  original no (una diferencia real) también va a `recursos/`, con el programa (el minificado) en
  `programa.cjs`: se repite con un tope mayor.
- **node no sobrevive a difuzz.** Cada node muere con SIGKILL si muere difuzz, también de golpe
  (SIGKILL, el OOM killer): `PR_SET_PDEATHSIG`, puesto con `setpriv --pdeathsig KILL` de
  util-linux si está en el `PATH` (si no, entre fork y exec, que cuesta más: +8 % de pared medido
  en 300 casos con `--jobs 8`, contra +2 %). La segunda SIGINT/SIGTERM, además, mata el grupo de
  cada node en curso antes de salir. Antes, un `while (true) {}` quedaba huérfano al 100 % de
  una CPU. No cubre a los procesos que lance el programa (sin `--sandbox`) si difuzz muere por
  SIGKILL, ni a los que hicieron `setsid` (el subreaper los mata después de cada ejecución, si
  difuzz sigue vivo).

## Códigos de salida

| código | significado |
|---|---|
| 0 | corrida entera (con o sin hallazgos; `CORRIDA.txt: completa`); `--cruce`, `--emitir` y `--help` sin error |
| 1 | error de entrada o de entorno: corpus inexistente, que no es directorio o sin `input.js`; `--out` que no se puede escribir; escrituras perdidas o casos sin contar; el oráculo se reescribió a mitad de corrida (ver [Compilar](#compilar)); `--cruce` sin índices; `--emitir` que no puede leer o minificar; stdout que no se puede escribir (disco lleno, `> /dev/full`; un pipe cerrado no) |
| 2 | error de uso: opción desconocida, valor o combinación inválida (con la ayuda en stderr); también `-h`, `--help` u otra opción de difuzz como valor de `--out`, `--cache`, `--indice`, `--emitir` o `--cruce` (para una ruta con ese nombre: `./-h`) |
| 130 | interrumpida por SIGINT o SIGTERM (`CORRIDA.txt: interrumpida`). Una segunda señal sale en el acto, mata los node en curso y deja `CORRIDA.txt: incompleta` |

Ni un pipe cerrado en stdout (`difuzz ... | head -1`) ni un error al escribir en stderr
(`2>&1 | rg -m1 DIVERGENCIA`) cambian el código: lo que no se pudo imprimir se pierde y la corrida
sigue, con sus resultados en `--out` y las stats en `CORRIDA.txt` (9.4; antes salía con 101 y,
con stderr cerrado a mitad de corrida, una divergencia podía perderse).

## Tests y verificaciones

Detalle en [`tests/README.md`](tests/README.md). Lo que corre difuzz de punta a punta necesita
`node`; el golden y la aceptación necesitan además el oráculo (los dos binarios compilados).

```sh
cargo test                                            # unitarios y de integración
DIFUZZ_SIN_NODE=1 cargo test --no-fail-fast           # sin node: saltea y cuenta los que lo necesitan
```

Sin `node` en el `PATH`, los tests que lo necesitan fallan con un mensaje claro. Con
`DIFUZZ_SIN_NODE=1` se saltean y cada uno deja una línea `DIFUZZ_SALTEADO_SIN_NODE` en stderr.

- **Golden de detección** (`tests/golden/`): 19 casos (17 con bugs conocidos de oxc y swc, el control `control-limpio` y una prueba del oráculo, `difuzz-senal-b`), en 8
  celdas, más `--invocar` y `--stubs`; la clase esperada de cada caso está en `esperado.tsv`.
- **Aceptación de los re-chequeos** (`tests/aceptacion/rechequeos/`): 181 casos que combinan un
  bug real de oxc con un flag de asunción (o con la forma en que se imprime), en las 4 celdas de oxc
  (724 comparaciones).
- **Corpus de codegen** (`tests/codegen/`, 10.1): 142 formas propias cuya semántica depende de la
  sintaxis exacta (paréntesis, comas, directivas, ASI, claves computadas, escapes), con `--crudo`
  en 8 celdas (1136 comparaciones). Las 9 del bug de los paréntesis de oxc van a `codegen/`; los
  bugs del minificador que también tocan (keepNames con `(0, function(){})`, la clase con
  `static ["prototype"]`), a `div/`. Destinos justificados en
  [`tests/codegen/README.md`](tests/codegen/README.md).

```sh
tests/golden/verificar.sh --jobs 4                    # sale con 0 y "comparaciones=N fallas=0"
tests/aceptacion/rechequeos/verificar.sh --jobs 4
tests/codegen/verificar.sh --jobs 4
cargo test --release --test verificaciones -- --ignored --test-threads=1   # los tres, como tests de cargo
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
2. `matriz.sh` y `medir-serie.sh` son ejecutables en el repo (modo 100755): se corren directo
(`scripts/matriz.sh ...`) o con `bash`. Una opción o un argumento de más también salen con 2, y una
celda en la que difuzz falló (`matriz.sh`) o un commit que no se pudo medir (`medir-serie.sh`: no
compila o su binario sale con error), con 1 (9.4; antes, 0).

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

## Corpus

difuzz recibe un directorio con archivos `input.js`. Los corpus de `tests/` sirven para verificar
difuzz, no para buscar bugs; para eso se usan tres, cada uno con un rol (decisión D2,
10-oct-2026):

| Corpus | Rol | De dónde sale | Cuándo se corre |
|---|---|---|---|
| Fixtures de SWC (`<clon de swc>/crates/swc_ecma_minifier/tests`, ~3100 casos) | regresión de difuzz y comparabilidad entre binarios (el arnés, `scripts/comparar-binarios.sh`; la muestra fija de `tests/muestra/` sale de acá) | ya existe | con cada cambio de difuzz |
| Sintético (`scripts/generar.cjs`) | exploración por familias de pasadas de oxc, y regresión de los bugs ya reportados | se genera en cada tanda, determinista por semilla | `regresion`: con cada versión nueva de oxc y antes de cada PR; `explorar`: al escribir o cambiar una familia |
| Código real (`scripts/armar-corpus-real.py`) | barrido periódico: patrones de código publicado que el generador no imagina | se arma con lo que ya está en el disco (caché de npm, `node_modules`) | con cada versión nueva de oxc |

**Sintético.** `scripts/barrido-sintetico.sh` genera la tanda y la corre (oxc, carril B por
defecto). La semilla es obligatoria y queda en `OUT/TANDA.txt` junto con el sha256 del árbol
generado (misma semilla, misma tanda, byte a byte):

```sh
scripts/barrido-sintetico.sh --semilla 59 regresion OUT/regresion [BIN]     # 1000 casos, --con-conocidos
scripts/barrido-sintetico.sh --semilla 1010 explorar F3 OUT/explorar-f3 [BIN] # 5000 casos de una familia
```

`regresion` genera a propósito los patrones de los bugs reportados: **tiene** que dar `div/`; un bug
que deja de aparecer está arreglado (o se rompió el generador). En `explorar` esos patrones no
salen, así que cada `div/` es candidato a bug nuevo. Una familia que da 0 bugs nuevos en 5000 casos
deja de explorarse y queda solo en la regresión. Las familias (F1-F10) y las opciones del generador
están en `node scripts/generar.cjs --help`.

**Código real.** `scripts/armar-corpus-real.py` lee las fuentes que se le pasan (no hace red),
filtra (tamaño, forma CJS/ESM, minificados, capacidades peligrosas, `require` dinámico), deduplica
por sha256, descarta lo que node no compila (Flow, JSX, TypeScript: un solo proceso de node que
compila sin ejecutar) y deja `fuentes.tsv` con el origen de cada caso. Un corpus por forma y por
tamaño:

```sh
F="--cacache $HOME/.npm/_cacache --dir OTRO_PROYECTO/node_modules"
scripts/armar-corpus-real.py $F --forma cjs --requires externos --neutralizar si --n 100000 --out CORPUS/cjs
scripts/armar-corpus-real.py $F --forma esm --requires externos --neutralizar si --n 100000 --out CORPUS/esm
# archivos de 30 KB a 3 MB, minificados incluidos: --min 30001 --max 3000000 --minificados
scripts/barrido-real.sh CORPUS/cjs OUT/cjs [BIN]    # el carril sale de fuentes.tsv: cjs -> a, esm -> b
```

`barrido-real.sh` corre la configuración que encontró bugs en código real (oxc, `--invocar
--stubs`) y fija `--sandbox-red`, sin opción para apagarlo: el corpus es código ajeno. El corpus
real no se versiona (las licencias son de sus autores); sí sus herramientas. Crecerlo bajando
paquetes de npm necesita red y queda para decidir.

Cuánto tarda (MEDIDO el 10-oct con `--jobs 8` y el binario de `fe0cd86`, antes del bloque 7, en
una máquina de 32 núcleos con carga 25-45 de otros procesos):

| Qué | Casos | Tiempo |
|---|---|---|
| `barrido-sintetico.sh regresion` | 1000 | 61 s |
| `barrido-sintetico.sh explorar F3` | 5000 | 144 s |
| `armar-corpus-real.py` (una pasada por ~33 000 archivos, con el pre-filtro) | — | ~56 s |
| `barrido-real.sh`, pool local entero (4 corpus: CJS y ESM, chicos y de 30 KB a 3 MB) | 18 749 | 23 min 35 s (~13 casos/s; CJS ~12/s, ESM ~25/s) |

Un `require` de un directorio (`require('..')`, `require('../')`) aparece en código real (por
ejemplo, los benchmarks de un paquete que cargan el paquete mismo). Con `--sandbox-red`, hasta 8.4 node lanzaba
`ERR_ACCESS_DENIED` al leer el `package.json` de ese directorio y el caso moría en el `require`, en
el original y en el minificado: con `fe0cd86` eran 968 de 18 749 casos del pool local en
`descartes.tsv` (causa `sandbox`); con 7.9, iguales sin correr el resto. Desde 8.4 ese `require` da
`MODULE_NOT_FOUND` como sin el sandbox (con `--stubs`, el stub; ver [Sandbox](#sandbox)). Medido
sobre esos 968 con la configuración de `barrido-real.sh`: cada caso queda en la misma clase que sin
el sandbox (939 iguales, 14 `enlace`, 11 `exit`, 2 `timeout` y 2 `observado_sin_salida` en
`descartes.tsv`; 0 `div/`), y `lanzaron` baja de 941 a 74 (los que antes morían en el `require` y
ahora corren el resto). Con 8.2 encima (un caso en que A y B mueren igual por un permiso negado sin
atrapar va a `sandbox_corto`, ver [Sandbox](#sandbox)) las cifras no cambian: `corto=0` en los 968.
En los primeros 500 casos del corpus CJS (`barrido-real.sh --limit 500`, MEDIDO el 11-oct), 8.2 solo
mandaba 32 casos de iguales a `sandbox_corto`, todos con un `require` de un directorio (`'..'`,
`'../..'`, `'.'`); con 8.4 y 8.2 juntos quedan 0 en `sandbox_corto`: 29 iguales y 3 `enlace`, lo mismo
que sin el sandbox.

Para triar un `div/` contra los bugs ya conocidos, el método que se usó es neutralizar cada familia
del catálogo (pasar el valor por una identidad opaca para oxc) y volver a correr: la familia que,
sola, hace que original y minificado coincidan es la causa.

## Licencia

difuzz todavía no tiene un archivo de licencia propio ni un campo `license` en `Cargo.toml`:
queda pendiente de que lo defina el dueño del repositorio.

Los casos de `tests/muestra/casos/` son archivos del proyecto
[SWC](https://github.com/swc-project/swc) copiados sin modificar, bajo licencia Apache 2.0
("Copyright 2024 SWC contributors"); el texto completo está en
[`tests/muestra/LICENSE-SWC`](tests/muestra/LICENSE-SWC) y el commit de origen en
`tests/muestra/README.md`.
