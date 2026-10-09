# Subir (o bajar) la version de oxc en difuzz

Procedimiento C7 de `interno/MEJORAS.md`. El trabajo lo hace `scripts/subir-oxc.sh`; este documento
dice cuando usarlo, como leer lo que deja y que queda a mano.

## Antes

1. Ver que hay una version nueva: `cargo search oxc_minifier --limit 1` (o
   `curl -s -A '<contacto>' https://crates.io/api/v1/crates/oxc_minifier`) y
   `curl -s https://registry.npmjs.org/oxc-minify/latest`. oxc publica los lunes, todos los crates
   `oxc_*` con el mismo numero. Que este tambien en npm importa: los reportes a oxc se reproducen con
   `oxc-minify` de npm. Ademas `probe/package.json` fija `oxc-minify` a la version exacta del crate
   (decision 9e): el test C1 de `tests/muestra/` exige que la version de npm sea la de
   `Minifier::Oxc.version()`, asi que el crate y el paquete de npm se suben juntos.
2. Partir de un commit (REV, default `HEAD`): lo no commiteado del arbol NO entra.
3. Tener compiladas las dependencias en algun target y pasarlo como `TARGET=` (si no, compila todo
   de cero en `DIR_SALIDA/target`, varios GB).

## Correr

```bash
TARGET=~/difuzz/target RED=1 scripts/subir-oxc.sh 0.154.0 ~/difuzz/.claude/trabajo-<dia>/oxc-0.154.0
```

- Tarda ~20-30 min (dos builds de cada perfil, `cargo test`, clippy, golden y aceptacion dos veces
  y el arnes en 5 celdas por lado). Anunciarlo antes (regla 9 de `CLAUDE.md`). `RED=1` (o `NPM=1`)
  usa la red: npm install (repro de `ENVIAR.md` y `oxc-minify` en `probe/`), C1 y `gh api`.
- Golden (`tests/golden/`), aceptacion de los re-chequeos (`tests/aceptacion/rechequeos/`) y arnes
  (`scripts/comparar-binarios.sh`) salen por defecto del arbol desde el que se llama (`REPO`); se
  cambian con `GOLDEN=`, `ACEPTACION=` y `ARNES=` (`GOLDEN=no`/`ACEPTACION=no` los saltean).
- Es retomable: si se corta o falla un paso, se vuelve a correr el MISMO comando y sigue desde el
  primer paso sin marca en `DIR_SALIDA/estado/`. Para rehacer un paso, borrar su `.ok` (y los que
  siguen). Despues del paso 11 el worktree ya no existe, pero los pasos 7-10 se pueden rehacer igual
  (usan `bin-base/`, `bin-nuevo/` y `arnes/`).
- **Si no compila (codigo 3)** es un cambio de API de oxc. Arreglar en `DIR_SALIDA/wt/src/` leyendo
  el fuente del crate nuevo (`~/.cargo/registry/src/*/oxc_<crate>-<version>/`), nunca de memoria, y
  volver a correr el mismo comando. Los arreglos quedan en el parche.
- Variables utiles: `BIN_BASE=` (binario ya compilado de REV, con su `-oraculo` al lado: saltea el
  build base), `JOBS=`, `CELDAS="oxc a;oxc b"`, `ARNES_ARGS="--base-de <salida vieja>/arnes/base"`,
  `GOLDEN=no`, `MANTENER=1` (no borra el worktree). Lista completa: `scripts/subir-oxc.sh --help`.

## Que deja en DIR_SALIDA

| Archivo | Que es |
|---|---|
| `subir-oxc.log` | lo que imprime cada paso; al final, la lista de cosas para revisar |
| `estado/revisar.txt` | esa lista sola (vacia = todo verde) |
| `INFORME.txt`, `diferencias.tsv` | el informe de diferencias por celda (ver abajo) |
| `triage/<celda>/<caso>/` | minificado de cada binario (`--emitir`) y su diff, para cada caso que cambia |
| `arnes/DIFERENCIAS.txt` | el detalle del arnes de no regresion (incluye minificados distintos en el mismo destino) |
| `golden/cambios.tsv`, `aceptacion/cambios.tsv` | celdas del golden y de la aceptacion de los re-chequeos que cambian de base a nuevo |
| `lock.diff`, `menciones-vieja.txt`, `api.stat` | paquetes del Cargo.lock que cambian; lineas que siguen nombrando la version vieja; archivos de src/ tocados |
| `subida-oxc-<version>.patch` | la propuesta: Cargo.toml, Cargo.lock, `version()`, el test del oraculo, `probe/package.json` (y `probe/package-lock.json` con `RED=1`) y los arreglos de API. Se aplica con `git apply` en el arbol principal (orden del usuario) |
| `bin-base/`, `bin-nuevo/` | los dos pares de binarios (`difuzz` y `difuzz-oraculo`) |
| `npm/` | `oxc-minify@<version>` y la salida de los repro de `ENVIAR.md` (con `RED=1`) |
| `logs/npm-probe.log`, `logs/c1.log` | la subida de `oxc-minify` en `probe/` y el test C1 de la muestra (con `RED=1`) |

## Como leer el informe de diferencias

Por celda, cada caso cuyo estado cambia. Estado = destinos del caso (`div:<kind>`, `known:<kind>`,
`panic`, `asserts`, `lint`, `recursos`), `descarte:<causa>` o `igual`.

- **DESAPARECE** (estaba en `div/` o `panic/` y ya no). Buena senal solo si es un fix real:
  - `-> igual` con minificado `cambio`: posible fix. Confirmar: buscar el commit/PR en oxc
    (`gh api repos/oxc-project/oxc/compare/crates_v<vieja>...crates_v<nueva>`) y correr el input
    minimo con `oxc-minify@<nueva>` de npm.
  - `-> descarte:*`: difuzz dejo de compararlo (p. ej. el parser nuevo rechaza el original). NO es
    un fix: el bug puede seguir ahi.
  - `-> known:*`: ahora lo explica una asuncion; revisar que el sensor tenga razon.
- **APARECE**: hallazgo nuevo; triagear como cualquier hallazgo (puede ser una regresion de oxc).
- **CAMBIA**: otro kind u otro destino (p. ej. `div:Stdout -> div:ThrowNew`, `panic -> asserts`).
- `sin_minificado(base|nuevo)`: `--emitir` falla en ese lado (no parsea o panic); no hay
  minificado que comparar. Si los dos estados son `descarte:*`, el caso sigue sin compararse y solo
  cambio la causa (ejemplo real: 0.152.0 -> 0.153.0, ver abajo).
- `minificado identico`: la diferencia no la causo oxc (no determinismo, timeouts por carga).
  Volver a correr esa celda antes de creer nada.
- El arnes ademas cuenta casos en el mismo destino con otro minificado: es lo esperable al cambiar
  de version y no se triagea uno por uno.
- Las celdas de swc son control: cambiar oxc no deberia moverlas. Si se mueven, es carga o algo de
  difuzz que depende de oxc (los flags estaticos se calculan con codigo comun).

## Lo que el script avisa y NO arregla

- **Golden y aceptacion** (`tests/golden/`, `tests/aceptacion/rechequeos/`): si oxc arreglo un bug
  del golden (o uno de los bugs que usa la aceptacion), la celda pasa de `ok` a `MAL` en el lado
  nuevo. Confirmado el fix, actualizar su `esperado.tsv` (y el README del golden) a mano (o sacar el
  caso). Si no hay fix confirmado, es una regresion.
- **`oxc-minify` en `probe/`**: el paso 3 cambia la version en `probe/package.json`; el
  `probe/package-lock.json` (y `probe/node_modules`) solo los sube el paso 10 con `RED=1`, que ademas
  corre C1 (`cargo test c1_igual_a_oxc_minify_de_npm -- --ignored`). Sin `RED=1` queda avisado: el
  parche lleva un `package.json` que no coincide con su lock hasta que se haga
  `npm install --save-exact oxc-minify@<version>` en `probe/`.
- **`ENVIAR.md`** (`.claude/trabajo-07oct/oxc/`): todos los textos dicen "0.153.0" y se reprodujeron
  con esa version. Antes de mandar cualquiera, reproducirlo con la version nueva (`RED=1` corre los
  `repro/*.mjs` y compara con sus `.out`) y cambiar la version del texto. El aviso sale en toda
  corrida cuya version vieja aparezca en `ENVIAR.md` (tambien al bajar, donde no aplica). Si un repro da otra salida,
  el bug pudo arreglarse: no mandarlo sin revisar.
- **Tests que dependen de bugs de oxc**: `subproceso.rs` `panic_del_minifier_cruza_el_pipe` usa el
  panic O6 (`"\ud800"["0"]`) para probar el camino de panic. Si oxc lo arregla, el test falla y hay
  que buscarle otro panic (no es una regresion de difuzz).
- **Comentarios con la version vieja** (`menciones-vieja.txt`): documentan con que version se verifico
  algo (`backend/oxc.rs` asunciones, `checks.rs` globals de oxc_ecmascript). No se cambian solos:
  hay que releer esa parte del fuente nuevo y recien ahi actualizar la cita.
- **Asunciones de oxc** (`crates/oxc_minifier/docs/ASSUMPTIONS.md`): si cambian, cambian las tablas
  de `backend/oxc.rs`. El archivo NO viaja en el crate publicado; con `RED=1` el script compara su
  blob entre los tags `crates_v<vieja>` y `crates_v<nueva>` (gh) y avisa si cambio.
- **Etiquetas de descarte que dependen del parser de oxc**: `backend/oxc.rs`, funcion `parses_as`.
  En el ejercicio de bajada (0.153.0 -> 0.152.0, b4-c) usaba solo el parser. Desde 0.153.0 (PR
  #27220, "reject module syntax in script") el parser rechaza `import`/`export` en script; en 0.152.0
  lo hacia la semantica. Por eso, en carril B, 5 casos con sintaxis de modulo pasaban de
  `parse_script` (0.152.0) a `parse_otro` (0.153.0). Desde R1-H06 (bloque 6) `parses_as` corre
  tambien la semantica (`with_check_syntax_error`, como `prepare`), asi que ese movimiento en
  particular no deberia repetirse (INFERENCIA, no medido con 0.152.0). No cambia ningun hallazgo, pero
  muestra que las causas de descarte pueden cambiar entre versiones.
- **El panic y el assert conocidos** (`string-index-utf16`, `issue_973/this_binding_collapse_vars`)
  llevan en `meta.txt` la ruta del registry con la version (`oxc_codegen-<v>/src/str.rs:712:52`):
  siempre aparecen como "mismo destino con otro contenido". Lo que importa es si cambia `archivo:linea`.
- **El oraculo** se recompila siempre con el principal (`cargo build --release && cargo build
  --profile oraculo`). El paso 5 verifica con `--preparar --info` que los cuatro binarios digan la
  version esperada; un oraculo viejo junto a un principal nuevo se rechaza en la corrida (b1-c).

## Despues (usuario)

Aplicar `subida-oxc-<version>.patch` en el arbol principal, volver a compilar los dos perfiles,
`npm ci` en `probe/` (para que `probe/node_modules` tenga el `oxc-minify` nuevo y C1 pase), y
anotar en `CLAUDE.md` (D1 nombra `oxc_minifier 0.153.0`) y en `interno/MEJORAS.md` la version nueva.
Si se confirmaron fixes de oxc, actualizar golden y `ENVIAR.md` en el mismo momento.
