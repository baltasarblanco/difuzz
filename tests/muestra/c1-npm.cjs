// 9e / C1: el minificado de difuzz contra `minifySync` de `oxc-minify` (npm)
// con las mismas opciones. Lo lanza el test `muestra::c1_*` (tests/muestra/auditoria.rs).
//
// uso: node c1-npm.cjs <node_modules> <dir de trabajo>
// Lee <trabajo>/pedidos.tsv: id, carril (a|b), preset, ruta del input.js.
// El minificado de difuzz de cada id esta en <trabajo>/<id>.difuzz.js.
// Escribe una linea por pedido en stdout:
//   id \t igual
//   id \t distinto \t byte <k> \t <contexto difuzz> \t <contexto npm>   (y deja <id>.npm.js)
//   id \t error_npm \t <mensajes>
//
// Opciones (las de src/backend/oxc.rs, Oxc::new):
// - core_defaults: MinifierOptions::default() con treeshake.annotations = false (M35).
// - carril_a: core_defaults con keep_names (funcion y clase) en compress y en
//   mangle, y mangle.top_level = true.
// El tipo de fuente sale del nombre: carril a -> "input.cjs" (CommonJS, como
// SourceType::cjs()), carril b -> "input.mjs" (modulo, SourceType::mjs()).
'use strict';
const fs = require('fs');
const path = require('path');

const [nm, trabajo] = process.argv.slice(2);
const { minifySync } = require(path.join(nm, 'oxc-minify'));

const keep = { function: true, class: true };
const OPCIONES = {
  core_defaults: { compress: { treeshake: { annotations: false } } },
  carril_a: {
    compress: { keepNames: keep, treeshake: { annotations: false } },
    mangle: { keepNames: keep, toplevel: true },
  },
};

const ctx = (s, k) => JSON.stringify(s.slice(Math.max(0, k - 40), k + 40));
const salida = [];
for (const linea of fs.readFileSync(path.join(trabajo, 'pedidos.tsv'), 'utf8').split('\n')) {
  if (!linea) continue;
  const [id, carril, preset, ruta] = linea.split('\t');
  const opciones = OPCIONES[preset];
  if (!opciones) throw new Error(`preset desconocido: ${preset}`);
  const src = fs.readFileSync(ruta, 'utf8');
  const r = minifySync(carril === 'a' ? 'input.cjs' : 'input.mjs', src, opciones);
  if (r.errors.length) {
    salida.push(`${id}\terror_npm\t${JSON.stringify(r.errors.map((e) => e.message))}`);
    continue;
  }
  const difuzz = fs.readFileSync(path.join(trabajo, `${id}.difuzz.js`), 'utf8');
  if (r.code === difuzz) {
    salida.push(`${id}\tigual`);
    continue;
  }
  let k = 0;
  while (k < difuzz.length && difuzz[k] === r.code[k]) k++;
  fs.writeFileSync(path.join(trabajo, `${id}.npm.js`), r.code);
  salida.push(`${id}\tdistinto\tbyte ${k}\t${ctx(difuzz, k)}\t${ctx(r.code, k)}`);
}
process.stdout.write(salida.join('\n') + '\n');
