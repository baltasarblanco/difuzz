// 9e: regenera (o verifica) la muestra fija del corpus de SWC en tests/muestra/.
//
// uso:
//   node tests/muestra/regenerar.cjs <corpus> [--escribir]
//   DIFUZZ_CORPUS=<corpus> node tests/muestra/regenerar.cjs [--escribir]
// <corpus> es <clon de swc>/crates/swc_ecma_minifier/tests, como el CORPUS de
// scripts/; sin argumento se usa DIFUZZ_CORPUS, y sin ninguno sale con 2.
// Sin --escribir solo VERIFICA: recalcula la seleccion y la compara contra
// LISTA.tsv y contra cada archivo copiado (sale con 1 si algo difiere).
// Con --escribir borra tests/muestra/casos/ y la vuelve a armar.
// El commit de origen sale de `git -C <corpus> rev-parse HEAD` y queda en la
// primera linea de LISTA.tsv.
//
// Criterio (v1, determinista, sin azar):
// - Casos: cada `input.js` bajo crates/swc_ecma_minifier/tests (sin seguir
//   symlinks, como corpus.rs) de hasta MAX_BYTES bytes.
// - Estrato: la carpeta de primer nivel (fixture, terser, full, mangle,
//   pass-1, pass-default). projects/ y benches-full/ no tienen `input.js`.
// - Tipo (solo para dar variedad, no es lo que usa difuzz): "modulo" si alguna
//   linea empieza con `import`/`export` como sentencia; si no, "script".
// - Orden dentro de cada (estrato, tipo): sha256("difuzz-muestra-v1\0" + ruta
//   relativa), ascendente. Se toman los primeros CUOTAS[estrato][tipo].
// - FIJOS: casos que entran siempre porque los tests los nombran (excepciones
//   conocidas de A1 en swc, del informe de la revision-1).
'use strict';
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { execFileSync } = require('child_process');

const SEMILLA = 'difuzz-muestra-v1\0';
const MAX_BYTES = 16 * 1024;
const CUOTAS = {
  fixture: { script: 24, modulo: 16 },
  terser: { script: 34, modulo: 8 },
  full: { script: 4, modulo: 2 },
  mangle: { script: 3, modulo: 1 },
  'pass-1': { script: 3, modulo: 1 },
  'pass-default': { script: 2, modulo: 0 },
};
const FIJOS = ['terser/compress/export/issue_2126', 'terser/compress/export/redirection'];

const args = process.argv.slice(2);
const escribir = args.includes('--escribir');
const SUBDIR = 'crates/swc_ecma_minifier/tests';
function errorDeUso(msj) {
  console.error(`${msj}\nuso: node tests/muestra/regenerar.cjs <corpus> [--escribir], o DIFUZZ_CORPUS=<corpus>; <corpus> = <clon de swc>/${SUBDIR}`);
  process.exit(2);
}
const raiz = args.find((a) => !a.startsWith('--')) || process.env.DIFUZZ_CORPUS || '';
if (!raiz) errorDeUso('falta el corpus de SWC');
if (!fs.existsSync(raiz) || !fs.statSync(raiz).isDirectory()) errorDeUso(`el corpus no es un directorio: ${raiz}`);
// Antes el argumento era la raiz del repo de swc: avisar en lugar de no encontrar casos.
if (fs.existsSync(path.join(raiz, SUBDIR))) errorDeUso(`${raiz} parece la raiz del repo de swc: el corpus es ${path.join(raiz, SUBDIR)}`);
const aqui = __dirname;
const casosDir = path.join(aqui, 'casos');
const listaPath = path.join(aqui, 'LISTA.tsv');

function recorrer(dir, out) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) recorrer(p, out);
    else if (e.isFile() && e.name === 'input.js') out.push(p);
  }
}

const sha = (s) => crypto.createHash('sha256').update(s).digest('hex');
// `import(` (dinamico) no cuenta: el caracter que sigue a `import` no puede ser `(`.
const esModulo = (src) => /^[ \t]*(import[ \t]*[{*"'\w]|export[ \t]*[{*\w])/m.test(src);

function seleccionar() {
  const todos = [];
  recorrer(raiz, todos);
  const cand = new Map(); // "estrato\ttipo" -> [{rel, clave}]
  const info = new Map();
  for (const abs of todos) {
    const rel = path.relative(raiz, path.dirname(abs)).split(path.sep).join('/');
    const src = fs.readFileSync(abs);
    const estrato = rel.split('/')[0];
    const tipo = esModulo(src.toString('utf8')) ? 'modulo' : 'script';
    info.set(rel, { estrato, tipo, bytes: src.length, sha: sha(src) });
    if (src.length > MAX_BYTES || FIJOS.includes(rel) || !CUOTAS[estrato]) continue;
    const k = `${estrato}\t${tipo}`;
    if (!cand.has(k)) cand.set(k, []);
    cand.get(k).push({ rel, clave: sha(SEMILLA + rel) });
  }
  const elegidos = [];
  for (const rel of FIJOS) {
    if (!info.has(rel)) throw new Error(`caso fijo ausente en el corpus: ${rel}`);
    elegidos.push({ rel, origen: 'fijo', ...info.get(rel) });
  }
  for (const [estrato, cuotas] of Object.entries(CUOTAS)) {
    for (const [tipo, n] of Object.entries(cuotas)) {
      const lista = (cand.get(`${estrato}\t${tipo}`) || []).sort((a, b) => (a.clave < b.clave ? -1 : a.clave > b.clave ? 1 : 0));
      if (lista.length < n) throw new Error(`cuota ${estrato}/${tipo}: hay ${lista.length}, se piden ${n}`);
      for (const { rel } of lista.slice(0, n)) elegidos.push({ rel, origen: 'cuota', ...info.get(rel) });
    }
  }
  elegidos.sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
  return { elegidos, total: todos.length };
}

const commit = execFileSync('git', ['-C', raiz, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
const { elegidos, total } = seleccionar();
const cabecera = `# swc ${commit} crates/swc_ecma_minifier/tests; criterio v1 (regenerar.cjs); ${elegidos.length} de ${total} casos\n`;
const lista =
  cabecera +
  'ruta\testrato\ttipo\torigen\tbytes\tsha256_input\tconfig_json\n' +
  elegidos
    .map((e) => {
      const conf = fs.existsSync(path.join(raiz, e.rel, 'config.json')) ? 'si' : 'no';
      return [e.rel, e.estrato, e.tipo, e.origen, e.bytes, e.sha, conf].join('\t');
    })
    .join('\n') +
  '\n';

if (escribir) {
  fs.rmSync(casosDir, { recursive: true, force: true });
  for (const e of elegidos) {
    const dst = path.join(casosDir, e.rel);
    fs.mkdirSync(dst, { recursive: true });
    for (const f of ['input.js', 'config.json']) {
      const src = path.join(raiz, e.rel, f);
      if (fs.existsSync(src)) fs.copyFileSync(src, path.join(dst, f));
    }
  }
  fs.writeFileSync(listaPath, lista);
  console.log(`escrita: ${elegidos.length} casos de ${total} (swc ${commit})`);
  process.exit(0);
}

// Verificar: la seleccion recalculada == LISTA.tsv, y cada archivo copiado ==
// el del corpus. La primera linea (commit) se compara aparte para avisar claro.
const errores = [];
const actual = fs.existsSync(listaPath) ? fs.readFileSync(listaPath, 'utf8') : '';
const [c0, ...r0] = actual.split('\n');
const [c1, ...r1] = lista.split('\n');
if (c0 !== c1) errores.push(`cabecera distinta (otro commit de swc?):\n  LISTA.tsv: ${c0}\n  calculada: ${c1}`);
if (r0.join('\n') !== r1.join('\n')) errores.push('la seleccion recalculada no coincide con LISTA.tsv');
for (const e of elegidos) {
  for (const f of ['input.js', 'config.json']) {
    const src = path.join(raiz, e.rel, f);
    const dst = path.join(casosDir, e.rel, f);
    const a = fs.existsSync(src) ? fs.readFileSync(src) : null;
    const b = fs.existsSync(dst) ? fs.readFileSync(dst) : null;
    if ((a === null) !== (b === null) || (a && !a.equals(b))) errores.push(`distinto: ${e.rel}/${f}`);
  }
}
if (errores.length) {
  console.error(errores.join('\n'));
  process.exit(1);
}
console.log(`ok: ${elegidos.length} casos coinciden con el corpus (swc ${commit})`);
