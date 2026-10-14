# Muestra fija del corpus (9e)

100 casos del corpus de tests del minifier de SWC, copiados sin modificar, para
que las auditorias A1, A2 y C1 corran dentro del repo sin depender de un clon
de SWC.

## Origen y licencia

- Proyecto: SWC, https://github.com/swc-project/swc
- Commit: `42e4f7e98b2c78d708a5b85c7e29365fdce2b213` (2026-09-22,
  "fix(es/minifier): mark properties assigned through destructuring targets as
  mutated (#12398)"), carpeta `crates/swc_ecma_minifier/tests/`.
- Licencia: Apache License 2.0, "Copyright 2024 SWC contributors". El texto
  completo esta en `LICENSE-SWC` (copia del `LICENSE` de la raiz de SWC en ese
  commit). En ese commit SWC no tiene `NOTICE` en la raiz ni en
  `crates/swc_ecma_minifier/`: el unico, `bindings/binding_native_addon/NOTICE`,
  atribuye a Node.js una interfaz de ese binding y no alcanza a estos casos.
- Archivos copiados: `input.js` y, cuando existe, `config.json` de cada caso,
  byte a byte, bajo `casos/<ruta relativa a crates/swc_ecma_minifier/tests>/`.
  No se modifico ninguno. difuzz no lee `config.json` (`src/` no lo nombra); se
  conserva para no separar el caso de su configuracion original.
- Algunos casos de SWC pueden ser fragmentos de codigo de terceros: SWC los
  guarda en carpetas con el nombre de un proyecto
  (`fixture/projects/{backbone,jquery,next,react,underscore,yui}/`,
  `fixture/next/`, `fixture/issues/{react,react-autosuggest,vercel}/`) y los
  distribuye sin avisos propios. Ningun archivo de `casos/` trae un aviso de
  copyright o de licencia.
- Los 44 casos de `casos/terser/` vienen, a traves de SWC, de la suite de
  tests de compresion de terser (https://github.com/terser/terser): SWC los
  declara "Tests ported from terser." (`tests/compress.rs`, sobre
  `tests/terser/compress/**/input.js`) y los genera con
  `scripts/x/terser/compress.js`, una version del runner de tests de terser que
  escribe cada test de `test/compress/<archivo>.js` en
  `<archivo>/<test>/{input.js,output.js,config.json}` (el `input.js` es la
  entrada del test impresa por terser). Terser se distribuye bajo la licencia
  BSD de 2 clausulas (BSD-2-Clause), "Copyright 2012-2018 (c) Mihai Bazon
  <mihai.bazon@gmail.com>". El texto completo esta en `LICENSE-TERSER` (copia
  del `LICENSE` del paquete npm `terser` 5.51.2; el de 5.17.6 dice lo mismo
  con una linea de encabezado mas). SWC no agrega un `NOTICE` ni un `LICENSE`
  propio para esos casos en el commit de arriba.

## Criterio de seleccion (v1)

Determinista, sin azar. Lo implementa `regenerar.cjs`:

1. Candidatos: cada `input.js` de hasta 16 KiB (en el commit de arriba, 3057
   de 3119; sin seguir symlinks, como `src/corpus.rs`).
2. Estrato: la carpeta de primer nivel. Tipo: `modulo` si alguna linea empieza
   con `import`/`export` como sentencia, si no `script` (solo para dar
   variedad; difuzz corre cada caso en los dos carriles).
3. Dentro de cada estrato y tipo, orden por
   `sha256("difuzz-muestra-v1\0" + ruta)`; se toman los primeros:

   | estrato | script | modulo |
   |---|---|---|
   | fixture | 24 | 16 |
   | terser | 34 | 8 |
   | full | 4 | 2 |
   | mangle | 3 | 1 |
   | pass-1 | 3 | 1 |
   | pass-default | 2 | 0 |

4. Mas 2 casos fijos que los tests nombran (las excepciones conocidas de A1 en
   swc): `terser/compress/export/issue_2126` y `terser/compress/export/redirection`.

`projects/` y `benches-full/` no tienen `input.js` y no entran.
`LISTA.tsv` tiene los 100 casos con estrato, tipo, bytes, sha256 del `input.js`
y si traen `config.json`; su primera linea dice el commit.

## Regenerar y verificar

```sh
node tests/muestra/regenerar.cjs CORPUS              # verifica: sale 1 si algo difiere
node tests/muestra/regenerar.cjs CORPUS --escribir   # borra casos/ y la rearma
```

`CORPUS` es `<clon de swc>/crates/swc_ecma_minifier/tests`, como en `scripts/`. Sin
argumento se usa `DIFUZZ_CORPUS`; sin ninguno de los dos sale con 2.

Con otro commit de SWC la seleccion puede cambiar (otros archivos, otros
hashes de contenido): la verificacion lo dice en la primera linea.

## Tests

Viven en `auditoria.rs`, incluido con `#[path]` desde `src/backend/oxc.rs`
(difuzz es solo binario; ver la cabecera del archivo).

```sh
cargo test muestra::                                   # A1 y A2 (sin node)
cargo test muestra::c1 -- --ignored --nocapture        # C1 (node + probe/node_modules)
```

- **A1** mismo printer: con compresion y mangle apagados, minificado == original.
  Excepciones: swc, carril B, `issue_2126` y `redirection` (`optimize` de SWC
  fusiona `export {...}` aun con todo apagado).
- **A2** original estable: re-imprimir el original da lo mismo. Sin excepciones.
- **C1** el minificado de oxc en difuzz == `minifySync` de `oxc-minify` (version
  exacta en `probe/package.json`, la misma que el crate) con las mismas
  opciones, presets `core_defaults` y `carril_a`. Sin excepciones. Necesita
  `npm ci` en `probe/`; sin node falla salvo `DIFUZZ_SIN_NODE=1`.

Una excepcion listada que deja de ocurrir tambien hace fallar el test.
