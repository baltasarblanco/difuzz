# Corpus de codegen (10.1)

142 formas cortas cuya semántica depende de la sintaxis exacta: paréntesis, comas, directivas,
ASI, claves computadas, escapes, `__proto__`, surrogates, `let`/`async`/`yield`/`await` como
identificadores, Annex B, módulo. Cada una imprime algo determinista. Sirven para el chequeo
`--crudo` de difuzz (el texto crudo contra el original que imprime el backend, A): en un corpus
cualquiera ese chequeo casi no encuentra nada (0 bugs en 24 933 ejecuciones de los fixtures de SWC
y del corpus real, medido el 11-oct), y en estas formas encontró el bug de los paréntesis de oxc.

## Origen y licencia

Las escribió el agente `codegen-fidelidad` el 11-oct-2026 (`formas.cjs`; informe en el trabajo
interno de ese día). Son propias del proyecto, no vienen de SWC ni de otro repositorio: van con la
licencia de difuzz. `LISTA.tsv` dice el tipo de fuente de cada una (`script` o `modulo`) y si se
adaptó.

**Adaptación (6 formas, `adaptada=si`):** las de módulo que leían su `export default` importándose
a sí mismas con `import * as m from "./input.mjs"` (el nombre del archivo en el arnés del agente)
usan `import(import.meta.url).then((m) => console.log(...))`: difuzz corre el programa con otro
nombre (`difuzz-<pid>-w<n>.mjs`), y el `import` estático no enlazaba. Sin `await` de primer nivel:
el módulo se importaría a sí mismo mientras evalúa y no terminaría. El resto, byte a byte, más un
salto de línea final.

## Tipo de fuente

difuzz no decide script o módulo por archivo sino por carril (`--carril a`: `.cjs`, script sloppy;
`--carril b`: `.mjs`, módulo). Las 9 formas de módulo se llaman `mod-*`. `verificar.sh` corre el
corpus entero en los dos carriles: las 7 `mod-*` con `import`/`export` o `await` de primer nivel
son `descarte:parse_esm` en el carril A con oxc, y las 12 de script que no parsean como módulo
(octales, `let`/`yield`/`await` como identificador, funciones en `if` y en etiquetas, `delete` de
un identificador) son `descarte:parse_script` en el carril B con oxc (swc rechaza o acepta
algunas distinto: ver `esperado.tsv`). Las que parsean en los dos carriles
se miden en los dos (`mod-this` y `mod-ne-paren` también corren como script).

## Ruido

Ninguna forma usa `Math.random`, `Date`, el pid, `Function.prototype.toString` ni stacks impresos.
Las 142 dieron el mismo resultado en dos corridas completas (8 celdas cada una). No hubo que
excluir ninguna.

## Qué se espera (`esperado.tsv`, 8 celdas con `--crudo`)

`verificar.sh` corre `oxc|swc × carril a|b × core_defaults|carril_a` con `--crudo` (142 × 8 = 1136
comparaciones). La clase de un caso con `codegen/` es `codegen:<kind>+<clase>`: el chequeo es
ortogonal y el caso sigue a la comparación de A con B como siempre.

oxc (`oxc_minifier` 0.153.0):

| formas | destino | por qué |
|---|---|---|
| `ne-assign-paren`, `-arrow`, `-class`, `ne-array-default-paren`, `ne-obj-default-paren`, `ne-logical-{or,and,nullish}-paren`, `mod-ne-paren` (9) | `codegen:Stdout` en las 4 celdas (+`known` en core_defaults, +`igual` en carril_a) | **O8**: el parser de oxc tira los paréntesis de `(h) = function(){}` y la función toma el nombre `"h"` (`""` en node). A y B lo cargan igual: es el punto ciego, solo `--crudo` lo ve. En core_defaults B además borra el nombre (asunción de `.name`, `known/`) |
| `kn-comma-assign`, `kn-comma-let`, `kn-comma-class`, `ne-var-init-comma` | `div:Stdout` en carril_a | **O9**: compress con keepNames quita el `0,` de `(0, function(){})` en una posición que nombra. En core_defaults no hay keepNames: B coincide por inline o la asunción de `.name` lo cubre |
| `kn-comma-default-destr`, `ne-prop-comma`, `ne-default-param-comma`, `mod-default-comma` | `div:Stdout` en las 4 (la `mod-` solo en el carril B) | O9 también en core_defaults: el re-chequeo de nombres (B con keepNames) sigue dando otra cosa, así que la asunción de `.name` no lo explica |
| `class-static-computed-prototype` | `div:Stdout` en las 4 | **O10**: compress borra una clase sin uso con `static ["prototype"]` y se pierde el `TypeError` |
| `lone-surrogate`, `sur-mid-charcode` | `div:Stdout` en las 4 | `"\uD800".charCodeAt(0)` da 65533: conocido upstream (oxc#15524, abierto) y sin asunción que lo cubra; queda en `div/` |
| `proto-shorthand` | `div:Stdout` en las 4 | `{ __proto__ }` con el binding renombrado (el issue C, ya enviado). Sin asunción |
| `ne-assign-noparen`, `ne-array-default-noparen`, `ne-obj-default-noparen`, `ne-var-init-paren-fn`, `stmt-start-fn-member`, `stmt-start-class` | `known` en core_defaults | asunción de `Function.prototype.name` (keepNames apagado) |
| `pure-call`, `pure-comma-call`, `pure-new`, `no-side-effects` | `igual` | difuzz apaga `treeshake.annotations` (M35): las anotaciones no se honran |

swc (los crates de `Cargo.lock`): las mismas 9 formas de O8 van a `codegen/` (la cadena de difuzz
corre `paren_remover` antes de imprimir A, como `@swc/core`), y además 8 formas que solo `--crudo` ve
con swc: `("use strict")` impreso como directiva (`dir-paren`, `dir-fn-paren`, `dir-arrow-body`,
carril A), `1 / /xy/` impreso `1//xy/` (`regex-after-paren`, A no compila), `(let)[0]` y
`for ((let) of …)` (`stmt-start-let-paren`, `let-for-of-paren`, A no compila), `(await x) ** 2`
impreso `await x**2` (`mod-await-exp`, carril B, A no compila) y `"\08"` en un módulo, que node
rechaza y swc acepta e imprime `"\x008"` (`dir-octal-before`, carril B: `ThrowLost`). Los casos en
que A no compila eran, sin `--crudo`, un descarte `invalido` silencioso. Las clases de A contra B en
swc son las que da su backend hoy (sin asunciones de oxc).

## Correr

```sh
tests/codegen/verificar.sh [--jobs N] [BINARIO] [DIR_SALIDA]    # sale con 0 y "comparaciones=1136 fallas=0"
cargo test --release --test verificaciones -- --ignored codegen
```

Necesita node y el oráculo sin debug assertions, como el golden (ver `tests/README.md`).
