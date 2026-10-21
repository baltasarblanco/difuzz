#!/usr/bin/env node
'use strict';
// generar.cjs: generador sintetico de programas dirigido a las pasadas de oxc_minifier
// (2026-10-09; uno de los tres corpus de difuzz, ver MANUAL.md, Corpus). Sin dependencias: solo node.
//
// uso: node scripts/generar.cjs DIR_SALIDA CANTIDAD [opciones]
//
//   --semilla N        semilla base (default 1). La del caso K es una funcion de N y K:
//                      misma semilla y mismo K, mismo programa (byte a byte)
//   --familias F1,..   familias a sortear por seccion (default F1..F10, ver abajo)
//   --desde K          numerar desde K (default 0): partir una tanda sin repetir casos
//   --con-conocidos    generar tambien los patrones de los bugs conocidos (A-D, #24331 y
//                      los del generador: E, F, G, H, I, K). Sin esto, un bug conocido inunda la tanda
//                      y tapa lo nuevo; con esto, la tanda es una suite de regresion
//   -h, --help         esta ayuda (sale con 0)
//
// Escribe DIR_SALIDA/NNNNN/input.js. Cada programa es deterministico por semilla, es valido
// como modulo ES y como script (empieza con "use strict": oxc no cuenta el modo sloppy como
// bug) y se auto-observa: cada seccion imprime su resultado y la traza de efectos (T) en orden.
// Familias: F1 sustitucion de uso unico, F2 inlining, F3 plegado de built-ins, F4 condiciones,
// F5 codigo muerto, F6 sin uso, F7 claves de propiedad, F8 ambitos, F9 plegado denso de
// built-ins, F10 evaluaciones de un mismo blanco (detalle en cada metodo FN de Gen).
// Para correr una tanda con difuzz: scripts/barrido-sintetico.sh.
//
// Respeta las asunciones de oxc (crates/oxc_minifier/docs/ASSUMPTIONS.md, 0.153.0):
// nada define toString/valueOf/Symbol.toPrimitive; ninguna funcion se convierte a
// primitivo (Function.prototype.toString); no hay Symbol coercionado, ni TDZ (cada
// variable se lee solo despues de su declaracion), ni `extends` de no-constructores,
// ni eval/with/arguments global, ni built-ins pisados, ni .name, ni .stack, ni Proxy,
// ni largos maximos de Array/String. Sin Date, Math.random ni timers.

const fs = require('fs');
// --con-conocidos: tambien genera los patrones de los bugs conocidos. Donde esta cada uno: A en un
// comentario de oxc#27412, B oxc#27834, C oxc#27863, D oxc#27864, el default con efecto en
// un rest en oxc#24331 (esta forma, https://github.com/oxc-project/oxc/issues/24331#issuecomment-6267846946), E oxc#27621, F oxc#27098, G oxc#25158,
// H oxc#27622, I oxc#27695 y oxc#27719, K oxc#27656.
const CONOCIDOS = process.argv.includes('--con-conocidos');
const path = require('path');

// ---------------------------------------------------------------------------
// PRNG (mulberry32)
function mulberry32(a) {
  return function () {
    a = (a + 0x6d2b79f5) | 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

class Gen {
  constructor(seed, familias) {
    this.r = mulberry32(seed);
    this.familias = familias;
    this.n = 0;
  }
  int(n) { return Math.floor(this.r() * n); }
  pick(a) { return a[this.int(a.length)]; }
  chance(p) { return this.r() < p; }
  id(p) { return `${p}${this.n++}`; }

  // --- literales -----------------------------------------------------------
  num() {
    return this.pick([
      '0', '1', '2', '3', '-1', '-0', '0.5', '-2.5', '10', '255', '1e21', '2147483648', '4294967295',
      '9007199254740993', 'NaN', 'Infinity', '-Infinity', '0.1', '1e-7', '0x10', '123456789', '-2147483649',
    ]);
  }
  str() {
    return this.pick([
      '""', '"a"', '"0"', '"1"', '"-1"', '"-0"', '"01"', '" 12 "', '"1e3"', '"0x1f"', '"abc"', '"NaN"',
      '"__proto__"', '"length"', '"p"', '"q"', '"true"', '"null"', '"\\u00e9"', '"a,b"',
    ]);
  }
  lit() {
    switch (this.int(8)) {
      case 0: case 1: case 2: return this.num();
      case 3: case 4: return this.str();
      case 5: return this.pick(['true', 'false']);
      case 6: return this.pick(['null', 'void 0', 'undefined']);
      default: return this.pick(['[]', '[1, 2]', '[0, "a", null]', '{}', '{ a: 1 }', '[,]', '[void 0]']);
    }
  }

  // --- expresiones -----------------------------------------------------------
  // sc: { prims: [nombres let/const/param con primitivos], lets: [asignables], objs: [objetos
  // con accesores], fns: [funciones que devuelven primitivos], arrs: [arrays de primitivos] }
  prim(sc) {
    if (sc.prims.length && this.chance(0.55)) return this.pick(sc.prims);
    return this.lit();
  }
  member(sc) {
    if (!sc.objs.length) return this.lit();
    const o = this.pick(sc.objs);
    switch (this.int(6)) {
      case 0: return `${o}.p`;
      case 1: return `${o}.q`;
      case 2: return `${o}?.r`;
      case 3: return `${o}["p"]`;
      case 4: return `${o}[${this.pick(['"q"', '"r"', '0', '"s"'])}]`;
      default: return `${o}.s`;
    }
  }
  expr(sc, d) {
    if (d <= 0) {
      const k = this.int(10);
      if (k < 5) return this.prim(sc);
      if (k < 8) return this.member(sc);
      if (k < 9 && sc.arrs.length) return `${this.pick(sc.arrs)}[${this.int(4)}]`;
      return this.lit();
    }
    const e = () => this.expr(sc, d - 1);
    switch (this.int(22)) {
      case 0: return `${this.pick(['!', '-', '+', '~', 'void ', 'typeof '])}${this.paren(e())}`;
      case 1: case 2: case 3:
        {
          const op = this.pick(['+', '+', '-', '*', '/', '%', '**', '&', '|', '^', '<<', '>>', '>>>']);
          // `-1 ** 2` y `typeof x ** 2` son SyntaxError: el operando izquierdo de ** va entre parentesis
          return op === '**' ? `((${e()}) ** ${e()})` : `(${e()} ${op} ${e()})`;
        }
      case 4: case 5:
        return `(${e()} ${this.pick(['==', '!=', '===', '!==', '<', '>', '<=', '>='])} ${e()})`;
      case 6: case 7: return `(${e()} ${this.pick(['&&', '||', '??'])} ${e()})`;
      case 8: return `(${e()} ? ${e()} : ${e()})`;
      case 9: return `(${e()}, ${e()})`;
      case 10: return `t(${JSON.stringify(this.id('k'))}, ${e()})`;
      case 11:
        if (sc.lets.length) return `(${this.pick(sc.lets)} ${this.pick(['=', '+=', '||=', '&&=', '??=', '-='])} ${e()})`;
        return e();
      case 12:
        if (sc.lets.length) return this.pick([`${this.pick(sc.lets)}++`, `--${this.pick(sc.lets)}`]);
        return e();
      case 13:
        if (sc.fns.length) return `${this.pick(sc.fns)}(${e()})`;
        return e();
      case 14: return `\`${this.pick(['', 'x', '-'])}\${${e()}}${this.pick(['', 'y'])}\${${e()}}\``;
      case 15: return this.member(sc);
      case 16:
        if (sc.objs.length) {
          const o = this.pick(sc.objs);
          return this.pick([`(${o}.p = ${e()})`, `(${o}.q ||= ${e()})`, `(${o}.s ??= ${e()})`, `delete ${o}.${this.pick(['q', 's', 'r'])}`, `("${this.pick(['p', 'q', 's'])}" in ${o})`]);
        }
        return e();
      case 17: return this.known(sc, d - 1);
      case 18:
        if (sc.arrs.length) {
          const a = this.pick(sc.arrs);
          return this.pick([`${a}.length`, `[...${a}].length`, `${a}.indexOf(${e()})`, `${a}.includes(${e()})`, `${a}.join(${this.pick(['', '"-"'])})`]);
        }
        return e();
      case 19: return `[${e()}, ${e()}].${this.pick(['length', 'join()', 'indexOf(1)', 'at(-1)', 'includes(void 0)'])}`;
      case 20: return `(${e()})?.${this.pick(['length', 'p', 'toFixed?.(1)', 'at?.(0)'])}`;
      default: return e();
    }
  }
  paren(s) { return /^[\w.$"`]+$/.test(s) ? s : `(${s})`; }
  sAscii() { return `(${this.pick(['""', '"a"', '"0"', '"abc"', '"a,b"', '" 12 "', '"NaN"', '"1e3"', '"p"'])})`; }
  // max: solo las que truncan a <= max (sin max, todas: el mismo pick de siempre)
  nFin(max = Infinity) { return `(${this.pick(['0', '1', '2', '3', '-1', '-0', '0.5', '-2.5', '10', '1e-7'].filter((p) => Math.trunc(Number(p)) <= max))})`; }
  // llamadas a metodos conocidos que oxc pliega (replace_known_methods / fold_constants)
  known(sc, d) {
    const e = () => this.expr(sc, d);
    // receptores siempre primitivos del tipo correcto (sin TypeError por null.charAt) y
    // entre parentesis (`10.toFixed` es SyntaxError)
    const s = () => (this.chance(0.6) ? `(${this.str()})` : `String(${this.prim(sc)})`);
    const n = () => (this.chance(0.6) ? `(${this.num()})` : `Number(${this.prim(sc)})`);
    switch (this.int(16)) {
      case 0: return `${this.paren(s())}.${this.pick(['charAt', 'charCodeAt', 'codePointAt', 'at'])}(${n()})`;
      // E/F (del generador): indexOf con no-ASCII (offset en bytes), posiciones >= 2**31, y
      // substring/slice con fin NaN/undefined se pliegan mal (oxc#27098; el no-ASCII, tambien oxc#25158); sin --con-conocidos esos
      // casos usan receptor ASCII literal y posiciones literales finitas. G (oxc#25158): en indexOf, con
      // busqueda "" y una posicion mas alla del largo del receptor (`("").indexOf((""), (2))` se
      // pliega a 2, node da 0); sin --con-conocidos indexOf toma solo las posiciones de nFin que no
      // pasan el largo de su receptor (lastIndexOf no diverge: nFin entera). Un solo pick, como antes.
      case 1: {
        const rec = CONOCIDOS ? this.paren(s()) : this.sAscii();
        const m = this.pick(['indexOf', 'lastIndexOf', 'includes', 'startsWith', 'endsWith']);
        // pos() solo corre sin --con-conocidos: rec es `("...")` de sAscii
        const pos = () => (m === 'indexOf' ? this.nFin(JSON.parse(rec.slice(1, -1)).length) : this.nFin());
        return `${rec}.${m}(${s()}${CONOCIDOS || this.chance(0.5) ? '' : `, ${pos()}`})`;
      }
      case 2: return `${CONOCIDOS ? this.paren(s()) : this.sAscii()}.${this.pick(['slice', 'substring', 'substr'])}(${CONOCIDOS ? n() : this.nFin()}${this.chance(0.5) ? `, ${CONOCIDOS ? n() : this.nFin()}` : ''})`;
      case 3: return `${this.paren(s())}.${this.pick(['toUpperCase', 'toLowerCase', 'trim', 'trimStart', 'trimEnd'])}()`;
      case 4: return `${this.paren(s())}.${this.pick(['replace', 'replaceAll'])}(${this.pick(['"a"', '"0"', '""', '"1"'])}, ${s()})`;
      case 5: return `${this.paren(s())}.concat(${e()}, ${e()})`;
      case 6: return `${this.paren(s())}.split(${this.pick(['""', '","', '"a"'])}).length`;
      case 7: return `Math.${this.pick(['abs', 'ceil', 'floor', 'round', 'sign', 'trunc', 'sqrt', ...(CONOCIDOS ? ['cbrt'] : []), 'fround', 'clz32', 'log2', 'expm1'])}(${n()})`;
      case 8: return `Math.${this.pick(['max', 'min', 'pow', 'imul', 'atan2', 'hypot'])}(${n()}, ${n()})`;
      case 9: return `${this.pick(['parseInt', 'parseFloat', 'Number', 'String', 'Boolean', 'isNaN', 'isFinite', 'Number.isInteger', 'Number.isSafeInteger', 'Number.isNaN'])}(${this.chance(0.5) ? s() : n()})`;
      case 10: return `parseInt(${s()}, ${this.pick(['2', '10', '16', '36', '0', '1'])})`;
      case 11: return `${this.paren(n())}.${this.pick(['toString(2)', 'toString(16)', 'toString()', 'toFixed(2)', 'toFixed()', 'toPrecision(3)', 'toExponential(1)'])}`;
      case 12: return `Number.${this.pick(['MAX_SAFE_INTEGER', 'MIN_SAFE_INTEGER', 'EPSILON', 'MAX_VALUE', 'MIN_VALUE', 'NaN', 'POSITIVE_INFINITY', 'NEGATIVE_INFINITY'])}`;
      case 13: return `String.fromCharCode(${this.pick(['65', '0', '65536', '-1', '0x10041', '97, 98'])})`;
      case 14: return `[${this.lit()}, ${this.lit()}, ${this.lit()}].${this.pick(['join()', 'join("")', 'toString()', 'length', 'indexOf(1)', 'lastIndexOf(void 0)', 'concat([3]).length', 'reverse().join()'])}`;
      default: return `Array.of(${n()}, ${s()}).join("|")`;
    }
  }

  // --- piezas de programa ---------------------------------------------------
  // Objeto con accesores que dejan rastro en T.
  objDecl(name, kw) {
    const p = this.lit();
    const parts = [`_p: ${p}`];
    // p: accesor con getter y setter (un getter solo haria TypeError al asignar en strict)
    if (this.chance(0.8)) {
      parts.push(`get p() { T.push("${name}.p"); return this._p; }`);
      parts.push(`set p(v) { T.push("${name}.p="); this._p = v; }`);
    } else parts.push(`p: ${this.lit()}`);
    parts.push(`q: ${this.lit()}`);
    if (this.chance(0.4)) parts.push(`get r() { T.push("${name}.r"); return ${this.lit()}; }`);
    if (this.chance(0.3)) parts.push(`s: void 0`);
    return `${kw} ${name} = { ${parts.join(', ')} };`;
  }

  // Una seccion: una funcion sN(a, b) con cuerpo de una familia, llamada e impresa.
  seccion(idx, fam) {
    const name = `s${idx}`;
    const sc = { prims: ['a', 'b'], lets: [], objs: [], fns: [], arrs: [] };
    const pre = [];
    // prologo: locales de tipos conocidos (declarados antes de usarse: sin TDZ)
    const nl = 1 + this.int(3);
    for (let i = 0; i < nl; i++) {
      const v = this.id('v');
      pre.push(`let ${v} = ${this.expr(sc, 1)};`);
      sc.prims.push(v);
      sc.lets.push(v);
    }
    if (this.chance(0.8)) {
      const o = this.id('o');
      pre.push(this.objDecl(o, this.pick(['const', 'let'])));
      sc.objs.push(o);
    }
    if (this.chance(0.4)) {
      const ar = this.id('ar');
      pre.push(`const ${ar} = [${this.lit()}, ${this.prim(sc)}, ${this.num()}];`);
      sc.arrs.push(ar);
    }
    if (this.chance(0.4)) {
      const f = this.id('f');
      const x = this.id('x');
      const inner = { ...sc, prims: [...sc.prims, x] };
      pre.push(`const ${f} = (${x}) => ${this.paren(this.expr(inner, 2))};`);
      sc.fns.push(f);
    }
    const body = this[fam](sc);
    const args = [this.prim({ prims: [] }), this.prim({ prims: [] })];
    return [
      `function ${name}(a, b) {`,
      ...pre.map((l) => `  ${l}`),
      ...body.map((l) => `  ${l}`),
      `}`,
      `P("${name}", ${name}(${args.join(', ')}));`,
      `P("${name}:T", T.splice(0).join());`,
    ];
  }

  // F1. Sustitucion de uso unico y reordenamiento (minimize_statements
  // substitute_single_use_symbol_*, inline.rs). Un let/const de uso unico con
  // inicializador con o sin efectos, sentencias intermedias con efectos, y un uso.
  F1(sc) {
    const out = [];
    const k = 1 + this.int(3);
    const names = [];
    const inits = () => {
      const o = sc.objs.length ? this.pick(sc.objs) : null;
      const opts = [
        () => this.expr(sc, 2),
        () => (o ? `${o}.p` : this.expr(sc, 1)),
        // spread de un objeto con getters de uso unico que se mueve: tambien Issue A (con identificador; comentario en oxc#27412)
        () => (o && CONOCIDOS ? `{ ...${o} }` : `[${this.expr(sc, 1)}]`),
        // spread de un literal con getter que se mueve al inlinear: Issue A (oxc#27412), solo con --con-conocidos
        () => (o && CONOCIDOS ? `{ ...{ get g() { T.push("sg"); return ${this.prim(sc)}; } } }` : this.lit()),
        () => `t("i", ${this.expr(sc, 1)})`,
        () => (sc.arrs.length ? `[...${this.pick(sc.arrs)}]` : `[${this.prim(sc)}]`),
        () => (sc.fns.length ? `${this.pick(sc.fns)}(${this.prim(sc)})` : this.prim(sc)),
        () => this.pick(sc.prims),
        () => (o ? `{ a: ${o}.q, b: ${this.prim(sc)} }` : `{ a: 1 }`),
        () => (o ? `${o}?.p ?? ${this.lit()}` : this.lit()),
        () => (o ? `[${o}.p, ${o}.q]` : this.lit()),
      ];
      return this.pick(opts)();
    };
    const decl = [];
    for (let i = 0; i < k; i++) {
      const n = this.id('u');
      names.push(n);
      decl.push(`${n} = ${inits()}`);
    }
    if (this.chance(0.5)) out.push(`${this.pick(['let', 'const'])} ${decl.join(', ')};`);
    else for (const dd of decl) out.push(`${this.pick(['let', 'const', 'var'])} ${dd};`);
    // intermedios con efectos sobre lo que lee el inicializador
    const nmid = this.int(3);
    for (let i = 0; i < nmid; i++) out.push(this.efecto(sc) + ';');
    // uso: cada nombre una sola vez, en una expresion que mezcla efectos
    const usos = names.map((n) => this.envolverUso(sc, n));
    const ret = this.pick([
      () => `return [${usos.join(', ')}];`,
      () => `return ${usos.join(' + ')};`,
      () => `P("u", ${usos.join(', ')}); return T.length;`,
      () => `return { ${usos.map((u, i) => `k${i}: ${u}`).join(', ')}${sc.objs.length ? `, ...${this.pick(sc.objs)}` : ''} };`,
      () => `if (${usos[0]}) return ${usos.slice(1).join(', ') || '1'}; else return ${usos.slice(1).join(', ') || '2'};`,
    ]);
    // en el ultimo caso los nombres podrian usarse dos veces: no importa (no es uso unico)
    out.push(ret());
    return out;
  }
  efecto(sc) {
    const o = sc.objs.length ? this.pick(sc.objs) : null;
    const opts = [
      () => (o ? `${o}.p = ${this.expr(sc, 1)}` : `t("m", 0)`),
      () => (sc.lets.length ? `${this.pick(sc.lets)} = ${this.expr(sc, 1)}` : `t("m", 0)`),
      () => `t("m", ${this.expr(sc, 1)})`,
      () => (o ? `delete ${o}.q` : `t("d", 0)`),
      () => (sc.lets.length >= 1 ? `[a, ${this.pick(sc.lets)}] = [${this.pick(sc.lets)}, a]` : `[a, b] = [b, a]`),
      () => (o ? `${o}.q = ${this.expr(sc, 1)}` : `b = a`),
      () => (sc.fns.length ? `${this.pick(sc.fns)}(${this.prim(sc)})` : `a++`),
      () => (o ? `({ p: ${this.pick(sc.lets.length ? sc.lets : ['b'])} } = ${o})` : `b--`),
    ];
    return this.pick(opts)();
  }
  envolverUso(sc, n) {
    return this.pick([
      () => n,
      () => `${this.expr(sc, 1)} + ${n}`,
      () => `${this.efecto(sc)}, ${n}`.replace(/^/, '(') + ')',
      () => `${n}?.${this.pick(['a', 'g', 'length', 'p'])}`,
      () => `\`\${${this.expr(sc, 0)}}\${${n}}\``,
      () => `t("w", ${this.expr(sc, 0)}) ? ${n} : ${this.lit()}`,
      () => `[${this.expr(sc, 1)}, ${n}]`,
      () => `typeof ${n}`,
      () => (sc.objs.length ? `(${this.pick(sc.objs)}.p = ${n})` : n),
      () => `(${this.pick(['a', 'b'])} = ${n})`,
    ])();
  }

  // F2. Inlining de constantes y valores frescos (inline.rs fresh_value_kind,
  // remove_unused_member_assignment): identidad, alias, escrituras, var con hoisting.
  F2(sc) {
    const out = [];
    const c = this.id('c');
    const kind = this.int(6);
    if (kind === 0) out.push(`const ${c} = { x: ${this.lit()}, y: ${this.expr(sc, 1)} };`);
    else if (kind === 1) out.push(`const ${c} = [${this.lit()}, ${this.lit()}];`);
    else if (kind === 2) out.push(`const ${c} = ${this.lit()};`);
    else if (kind === 3) out.push(`const ${c} = function () { T.push("cf"); return ${this.lit()}; };`);
    else if (kind === 4) out.push(`class ${c} { static z = t("cz", ${this.lit()}); m() { return ${this.lit()}; } }`);
    else out.push(`const ${c} = ${this.str()};`);
    const d = this.id('d');
    const ops = [
      () => `const ${d} = ${c}; ${d}.w = ${this.lit()}; P("al", ${c}.w, ${d} === ${c});`,
      () => `${c}.w = t("ww", ${this.lit()});`,
      () => `P("id", ${c} === ${c}, typeof ${c}, ${c} == ${this.lit()});`,
      () => `if (${this.expr(sc, 1)}) { P("br", ${c}); }`,
      () => `P("len", ${c}?.length, ${c}?.x);`,
      () => `var ${d}; P("vh", ${d}); ${d} = ${this.expr(sc, 1)}; P("vh2", ${d});`,
      () => `P("pre", typeof ${d}); var ${d} = ${this.lit()};`,
      () => `if (${this.expr(sc, 1)}) var ${d} = ${this.lit()}; P("cv", ${d});`,
      () => `let ${d} = ${c}; ${d} = ${this.lit()}; P("re", ${d}, ${c} === ${d});`,
      () => `P("ks", Object.keys(typeof ${c} === "object" && ${c} !== null ? ${c} : {}).join());`,
      () => `const ${d} = { ...(typeof ${c} === "object" ? ${c} : {}), v: ${this.lit()} }; P("sp", ${d});`,
    ];
    const n = 2 + this.int(3);
    const used = new Set();
    // con un primitivo, escribir una propiedad es TypeError en strict: esas ops pasan a la 2
    const primitivo = kind === 2 || kind === 5;
    for (let i = 0; i < n; i++) {
      let j = this.int(ops.length);
      if (primitivo && (j === 0 || j === 1)) j = 2;
      if ((j === 5 || j === 6 || j === 7 || j === 0 || j === 8 || j === 10) && used.has('d')) j = 2;
      if (j === 5 || j === 6 || j === 7 || j === 0 || j === 8 || j === 10) used.add('d');
      out.push(ops[j]());
    }
    out.push(`return ${this.pick([c + ' === ' + c, `typeof ${c}`, `[${this.expr(sc, 1)}]`, `${this.expr(sc, 2)}`])};`);
    return out;
  }

  // F3. Plegado de constantes y metodos conocidos (fold_constants, replace_known_methods,
  // substitute_alternate_syntax): numeros raros, -0, NaN, strings numericos, typeof.
  F3(sc) {
    const out = [];
    const n = 3 + this.int(5);
    const vals = [];
    const lit = { prims: [], lets: [], objs: [], fns: [], arrs: [] };
    for (let i = 0; i < n; i++) {
      const src = this.chance(0.6) ? lit : sc;
      const k = this.int(5);
      let e;
      if (k === 0) e = this.known(src, 1);
      else if (k === 1) e = `typeof ${this.paren(this.expr(src, 1))} ${this.pick(['===', '==', '!==', '!='])} "${this.pick(['undefined', 'number', 'string', 'object', 'boolean', 'function', 'bigint', 'symbol'])}"`;
      else if (k === 2) e = this.pick([`${this.pick(['1n', '(-2n)', '0n', '10n'])} ${this.pick(['+', '*', '-', '**', '%', '<<', '&', '>>'])} ${this.pick(['1n', '3n', '64n'])}`, `${this.pick(['1n', '0n'])} ${this.pick(['==', '===', '<', '>='])} ${this.pick(['1', '"1"', '0', 'false', '1n'])}`, `typeof ${this.pick(['1n', 'BigInt(2)'])}`]);
      else e = this.expr(src, 2 + this.int(2));
      vals.push(e);
    }
    out.push(`const r = [];`);
    for (const e of vals) out.push(`r.push(${e});`);
    out.push(`return r;`);
    return out;
  }

  // F4. Condiciones, logicos, not y contexto booleano (minimize_conditions,
  // minimize_conditional_expression, minimize_logical_expression, minimize_not_expression,
  // minimize_if_statement): de if a ?:, a && / ||, a ||= ; x == null; typeof undefined.
  F4(sc) {
    const out = [];
    const tgt = () => (sc.objs.length && this.chance(0.5) ? `${this.pick(sc.objs)}.${this.pick(['p', 'q', 's'])}` : (sc.lets.length ? this.pick(sc.lets) : 'a'));
    const c = () => this.pick([
      () => this.expr(sc, 2),
      () => `${this.pick(sc.prims)} == null`,
      () => `${this.pick(sc.prims)} === null || ${this.pick(sc.prims)} === void 0`,
      () => `typeof ${this.pick(sc.prims)} === "undefined"`,
      () => `!(${this.expr(sc, 1)} && ${this.expr(sc, 1)})`,
      () => `!(${this.expr(sc, 1)} || !${this.paren(this.expr(sc, 1))})`,
      () => `typeof ${this.pick(sc.prims)} == "object" && ${this.pick(sc.prims)} !== null`,
    ])();
    const n = 2 + this.int(4);
    for (let i = 0; i < n; i++) {
      const k = this.int(10);
      const x = tgt();
      if (k === 0) out.push(`if (${c()}) ${x} = ${this.expr(sc, 1)}; else ${x} = ${this.expr(sc, 1)};`);
      else if (k === 1) out.push(`${x} = ${x} ${this.pick(['||', '&&', '??'])} ${this.expr(sc, 1)};`);
      else if (k === 2) out.push(`${x} || (${x} = ${this.expr(sc, 1)});`);
      else if (k === 3) out.push(`if (${c()}) t("then", 1);`);
      else if (k === 4) out.push(`if (!${this.paren(c())}) { t("n1", 0); } else { t("n2", 0); }`);
      else if (k === 5) out.push(`P("c", ${c()} ? ${this.expr(sc, 1)} : ${c()} ? ${this.expr(sc, 1)} : ${this.expr(sc, 1)});`);
      else if (k === 6) out.push(`if (${c()}) { if (${c()}) t("ii", 1); } else if (${c()}) t("ie", 2);`);
      else if (k === 7) out.push(`${x} = ${x} ${this.pick(['+', '-', '*', '|', '&', '**'])} ${this.pick(['1', this.expr(sc, 0)])};`);
      else if (k === 8) out.push(`P("b", !!(${c()}), ${c()} ? true : false, ${this.expr(sc, 1)} ? ${x} : ${x});`);
      else out.push(`if (${c()}) return ${this.expr(sc, 1)};`);
    }
    out.push(`return ${this.pick([`${c()} ? ${this.expr(sc, 1)} : ${this.expr(sc, 1)}`, this.expr(sc, 2), `[${tgt()}, ${tgt()}]`])};`);
    return out;
  }

  // F5. Flujo de control (remove_dead_code, minimize_switch_statements,
  // minimize_for_statement, handle_*_statement, try_fold_labeled): switch con caida,
  // labels, break/continue, codigo muerto con var/funciones izadas, for-in/of.
  F5(sc) {
    const out = [];
    const n = 1 + this.int(3);
    for (let i = 0; i < n; i++) {
      const k = this.int(9);
      if (k === 0) {
        const cases = [];
        const nc = 1 + this.int(4);
        for (let j = 0; j < nc; j++) {
          const lab = this.chance(0.3) ? `t("cs", ${this.lit()})` : this.lit();
          cases.push(`case ${lab}: t("c${j}", 0);${this.chance(0.6) ? ' break;' : ''}`);
        }
        if (this.chance(0.6)) cases.splice(this.int(cases.length + 1), 0, `default: t("df", 0);${this.chance(0.5) ? ' break;' : ''}`);
        out.push(`switch (${this.expr(sc, 1)}) { ${cases.join(' ')} }`);
      } else if (k === 1) {
        const L = this.id('L');
        out.push(`${L}: { t("l1", 0); if (${this.expr(sc, 1)}) break ${L}; t("l2", 0); }`);
      } else if (k === 2) {
        const L = this.id('L');
        const i1 = this.id('i');
        const j1 = this.id('j');
        out.push(`${L}: for (let ${i1} = 0; ${i1} < 3; ${i1}++) { for (let ${j1} = 0; ${j1} < 3; ${j1}++) { if (${this.expr({ ...sc, prims: [...sc.prims, i1, j1] }, 1)}) ${this.pick(['continue', 'break'])} ${L}; t("x" + ${i1} + ${j1}, 0); } }`);
      } else if (k === 3) {
        const z = this.id('z');
        out.push(`if (${this.pick(['false', '0', '""', 'null', this.expr(sc, 1)])}) { var ${z} = ${this.lit()}; } P("hz", ${z});`);
      } else if (k === 4) {
        const g = this.id('g');
        out.push(`P("fd", ${g}()); if (${this.expr(sc, 1)}) { t("early", 0); } function ${g}() { return ${this.lit()}; }`);
      } else if (k === 5) {
        const kk = this.id('kk');
        const src = sc.objs.length ? this.pick(sc.objs) : `{ a: 1, 0: 2, b: 3 }`;
        out.push(`for (const ${kk} in ${src}) { if (${kk} === "${this.pick(['q', 'p', '_p', 's'])}") ${this.pick(['continue', 'break'])}; T.push(${kk}); }`);
      } else if (k === 6) {
        const e = this.id('e');
        const arr = sc.arrs.length ? this.pick(sc.arrs) : `[${this.lit()}, ${this.lit()}]`;
        out.push(`for (const ${e} of ${arr}) { if (${this.expr({ ...sc, prims: [...sc.prims, e] }, 1)}) break; P("fo", ${e}); }`);
      } else if (k === 7) {
        const w = this.id('w');
        out.push(`let ${w} = 0; while (${w} < 3) { ${w}++; if (${this.expr({ ...sc, prims: [...sc.prims, w] }, 1)}) continue; t("wh" + ${w}, 0); }`);
      } else {
        const w = this.id('w');
        out.push(`let ${w} = 0; do { t("do", ${w}); } while (++${w} < ${this.pick(['0', '1', '2'])} && ${this.expr({ ...sc, prims: [...sc.prims, w] }, 1)});`);
      }
    }
    out.push(this.pick([
      `return ${this.expr(sc, 1)};`,
      `if (${this.expr(sc, 1)}) { return 1; } else { return 2; } t("dead", 0);`,
      `try { return ${this.expr(sc, 1)}; } finally { t("fin", 0); }`,
      `{ return T.length; var dd = 1; }`,
    ]));
    return out;
  }

  // F6. Eliminacion de expresiones y declaraciones sin uso (remove_unused_expression,
  // remove_unused_declaration, remove_unused_class): sentencias-expresion con getters,
  // spreads, templates, clases con bloques estaticos, objetos sin uso.
  F6(sc) {
    const out = [];
    const o = sc.objs.length ? this.pick(sc.objs) : null;
    const n = 2 + this.int(4);
    for (let i = 0; i < n; i++) {
      const opts = [
        () => (o ? `${o}.${this.pick(['p', 'r', 'q'])};` : `t("e", 0);`),
        () => (o ? `({ ...${o} });` : `[...[1]];`),
        () => (o ? `[...[${o}.p]];` : `[1];`),
        () => (o ? `\`\${${o}.p}\`;` : `\`\`;`),
        () => (o ? `void ${o}.p;` : `void 0;`),
        () => (o ? `(${o}.r, ${this.lit()});` : `0;`),
        () => (o ? `typeof ${o}.p;` : `typeof a;`),
        () => (o ? `!${o}.p;` : `!a;`),
        () => (o ? `${o}?.p ?? t("nn", 0);` : `a ?? t("nn", 0);`),
        () => `class ${this.id('K')} { static { t("sb", 0); } }`,
        () => `class ${this.id('K')} { static f = t("sf", ${this.lit()}); }`,
        () => `class ${this.id('K')} { [t("ck", "${this.pick(['m', 'n', '1'])}")]() {} }`,
        () => `(class { static [t("ck2", "z")] = 1; });`,
        () => `let ${this.id('un')} = t("un", ${this.expr(sc, 1)});`,
        () => `({ a: t("oa", 1), b: ${o ? `${o}.p` : '2'} });`,
        () => `[t("aa", 1), ${this.expr(sc, 1)}];`,
        () => `(() => t("iife", 1))();`,
        () => `${this.expr(sc, 2)};`,
        () => (o ? `const { p: ${this.id('dp')}, ...${this.id('rs')} } = ${o};` : `const [${this.id('dq')}] = [1];`),
        () => `const ${this.id('fo')} = { w: 1 }; `,
        () => {
          const u = this.id('uo');
          return `const ${u} = {}; ${u}.k = t("pw", 1); ${u}.j = ${this.expr(sc, 1)};`;
        },
        () => `new (class { constructor() { t("ctor", 0); } })();`,
        () => (o ? `${o}.p === ${o}.p;` : `a === a;`),
        () => `(${this.expr(sc, 1)}) in { a: 1 };`,
      ];
      out.push(this.pick(opts)());
    }
    out.push(`return ${this.expr(sc, 1)};`);
    return out;
  }

  // F7. Claves de propiedad y formas de literales (substitute_alternate_syntax
  // try_compress_property_key, fold_object_exp, convert_to_dotted_properties, codegen):
  // claves computadas numericas/negativas/raras, __proto__, clases con miembros
  // computados, privados, destructuring con claves computadas.
  F7(sc) {
    const out = [];
    // Claves numericas negativas computadas en destructuring/clases son el Issue B (oxc#27834): en esos
    // contextos solo con --con-conocidos (en literales de objeto se imprimen bien).
    // b4: tambien [Infinity] (se pliega a 1/0 sin corchetes) y, en metodos/accesores de
    // literales de objeto, cualquiera de estas (variantes del Issue B que encontro el generador).
    const NEG = ['[-1]', '[-0]', '[-1e-7]', '[-Infinity]', '[-(1)]', '[Infinity]'];
    const BASE = [
      '[0]', '[1.5]', '[1e21]', '["-1"]', '["01"]', '["1"]', '["a b"]', '["__proto__"]',
      '["constructor"]', '[0x10]', '[NaN]', '["1e3"]', '[2 ** 53]', '["\\u00e9"]',
      '["length"]', '["0.5"]', '[1/0]', '["" + 1]', '[`k`]', '["p"]', '["-0"]', '[.5]', '[5e-324]', '[1 - 1]', '[0 / 1]',
    ];
    const keyObj = () => this.pick([...BASE, ...NEG]);
    const key = () => this.pick(CONOCIDOS ? [...BASE, ...NEG] : BASE);
    const n = 2 + this.int(3);
    for (let i = 0; i < n; i++) {
      const k = this.int(9);
      const v = this.id('ob');
      if (k === 0) {
        const props = [];
        for (let j = 0; j < 1 + this.int(3); j++) props.push(`${keyObj()}: ${this.lit()}`);
        out.push(`const ${v} = { ${props.join(', ')} }; P("ok", Object.keys(${v}).join("|"), ${v});`);
      } else if (k === 1) {
        const x = this.id('dx');
        const kk = key();
        out.push(`const { ${kk}: ${x} = ${this.lit()} } = { ${kk.replace(/^\[|\]$/g, '').startsWith('"') ? kk : kk}: ${this.lit()} }; P("dk", ${x});`);
      } else if (k === 2) {
        const x = this.id('ax');
        out.push(`let ${x}; ({ ${key()}: ${x} } = { "-1": 1, "0": 2, "1": 3, "01": 4, "1e+21": 5, "Infinity": 6, "p": 7 }); P("ak", ${x});`);
      } else if (k === 3) {
        const C = this.id('C');
        const kk = key();
        out.push(`class ${C} { ${kk} = ${this.lit()}; static ${key()}() { return ${this.lit()}; } get ${key()}() { return 7; } } P("ck", Object.keys(new ${C}()).join("|"), Object.getOwnPropertyNames(${C}).filter((n) => n !== "length" && n !== "name" && n !== "prototype").join("|"));`);
      } else if (k === 4) {
        const pr = this.id('__proto__x');
        const which = this.int(4);
        // `const __proto__` + `{ __proto__ }` es el Issue C (oxc#27863): solo con --con-conocidos
        if (which === 0 && CONOCIDOS && !this.yaProto) { this.yaProto = true; out.push(`const __proto__ = ${this.lit()}; P("pp", Object.keys({ __proto__ }));`); }
        else if (which === 0) out.push(`P("pp0", Object.keys({ "__proto__": ${this.lit()}, ["__proto__"]: 2 }));`);
        else if (which === 1) out.push(`P("pp2", Object.getPrototypeOf({ __proto__: null }) === null, Object.keys({ ["__proto__"]: 1 }), Object.keys({ "__proto__": {} }));`);
        else if (which === 2) out.push(`const ${pr} = { ...{ ["__proto__"]: ${this.lit()} } }; P("pp3", Object.keys(${pr}));`);
        else out.push(`const ${pr} = { __proto__() { return 1; } }; P("pp4", Object.keys(${pr}), typeof ${pr}.__proto__);`);
      } else if (k === 5) {
        const sp = this.id('sp');
        out.push(`const ${sp} = { a: 1, ...{ ${keyObj()}: ${this.lit()}, b: ${this.expr(sc, 1)} }, ...${this.pick(['null', 'void 0', '"ab"', '[7, 8]', '1', 'true', `{ ${keyObj()}: 2 }`])} }; P("sp", ${sp});`);
      } else if (k === 6) {
        const C = this.id('Cp');
        out.push(`class ${C} { #x = ${this.lit()}; static has(o) { return #x in o; } get x() { return this.#x; } set x(v) { this.#x = v; } } { const i = new ${C}(); i.x = ${this.lit()}; P("pv", i.x, ${C}.has(i), ${C}.has({})); }`);
      } else if (k === 7) {
        const ob = this.id('dd');
        out.push(`const ${ob} = { ${this.pick(['"a-b"', '"1"', '"a"', '"if"', '"class"', '"0"'])}: 1 }; P("dot", ${ob}["a-b"], ${ob}["1"], ${ob}["a"], ${ob}["if"], ${ob}["class"], ${ob}[0]);`);
      } else {
        const m = this.id('m');
        out.push(`const ${m} = { ${key()}() { return 1; }, get ${key()}() { return 2; }, set ${key()}(v) { T.push("st"); } }; P("mk", Object.getOwnPropertyNames(${m}).join("|"));`);
      }
    }
    out.push(`return ${this.expr(sc, 1)};`);
    return out;
  }

  // F8. Ambitos y funciones (substitute_catch_clause, try_remove_name_from_functions,
  // substitute_iife_call, try_rewrite_arguments_copy_loop, rest/default params,
  // let por iteracion, generadores).
  F8(sc) {
    const out = [];
    const n = 1 + this.int(3);
    for (let i = 0; i < n; i++) {
      const k = this.int(11);
      const f = this.id('h');
      // `catch (h) { var h }` con h = nombre de la function expression es el Issue D (oxc#27864): solo con --con-conocidos
      if (k === 0) out.push(`P("nfe", (function ${f}() { ${CONOCIDOS ? this.pick([`${f} = 1;`, '']) : ''} try { t("ty", 0); } catch (${f}) { ${this.pick([...(CONOCIDOS ? [`var ${f};`] : []), `${f} = 2;`, ''])} } return typeof ${f}; })());`);
      else if (k === 1) out.push(`P("iife", (function (x, y) { return ${this.pick(['x + y', '[x, y]', 'arguments.length', 'y', 'typeof y'])}; })(${this.expr(sc, 1)}${this.chance(0.5) ? `, ${this.expr(sc, 1)}` : ''}));`);
      else if (k === 2) out.push(`P("arr", ((x = t("dx", ${this.lit()}), y = x) => [x, y])(${this.pick(['', 'void 0', this.expr(sc, 1)])}));`);
      // default con efecto dentro de un rest es de la familia de oxc#24331 (esta forma, https://github.com/oxc-project/oxc/issues/24331#issuecomment-6267846946): solo con --con-conocidos
      else if (k === 3) out.push(`P("rest", (function (...[r1 = ${CONOCIDOS ? 't("rd", 1)' : '1'}, ...r2]) { return [r1, r2.length]; })(${this.pick(['', '0', 'void 0, 2', this.expr(sc, 1)])}));`);
      else if (k === 4) out.push(`P("ac", (function () { var r = Array(arguments.length); for (var ix = 0; ix < arguments.length; ix++) r[ix] = arguments[ix]; return r; })(${this.lit()}, ${this.lit()}));`);
      else if (k === 5) out.push(`{ const fs = []; for (let ii = 0; ii < 3; ii++) { fs.push(() => ii); ${this.pick(['', 'ii++;', 'if (ii) continue;'])} } P("lp", fs.map((g) => g())); }`);
      else if (k === 6) out.push(`P("gen", [...(function* () { yield t("g1", 1); ${this.pick(['yield* [2, 3];', 'return 4;', 'yield;'])} yield ${this.lit()}; })()]);`);
      else if (k === 7) out.push(`try { ${this.pick(['null.x;', 'undefined();', 'throw 1;', `throw new RangeError("r");`, 't("ok", 0);'])} } catch ${this.pick(['(e)', '(e)', '{ var e = "nb"; }'])} ${this.pick(['{ P("ce", typeof e === "object" ? e.constructor === TypeError : e); }', '{ t("cc", 0); }'])}`.replace('catch { var e = "nb"; } ', 'catch { var e = "nb"; } ') );
      else if (k === 8) out.push(`P("cl", (() => { let c = 0; const inc = () => ++c; inc(); ${this.pick(['inc();', ''])} return c; })());`);
      else if (k === 9) out.push(`P("th", function () { return (() => this)(); }.call(${this.pick(['1', '"s"', 'null', 'void 0'])}));`);
      else out.push(`P("ar2", ((x) => { var x; return x; })(${this.lit()}), (function (x) { function x() {} return typeof x; })(${this.lit()}));`);
    }
    out.push(`return ${this.expr(sc, 1)};`);
    return out;
  }

  // F9. Plegado denso de built-ins con argumentos de borde (oxc_ecmascript
  // constant_evaluation, replace_known_methods, fold_constants). Agregada despues de t5k:
  // F3 fue la familia que rindio (E y F). Cada expresion es independiente y se imprime en
  // un array: la divergencia queda ubicada por su indice.
  F9(sc) {
    const A = ['undefined', 'NaN', '-0', '0', '0.5', '-0.5', '1.5', '-1', '2', '3', '1e21', '2 ** 31', '2 ** 32', '2 ** 32 + 1', 'Infinity', '-Infinity', '"2"', '" 3 "', 'null', 'true', '[]', '[1]', '"0x10"', '""', '5e-324', '-1e-7', '255', '36', '37', '1'];
    const Afin = A.filter((x) => !/^(undefined|NaN|2 \*\* 3|2 \*\* 32)/.test(x) || CONOCIDOS);
    const S = ['""', '"a"', '"abc"', '"aBc"', '" a b "', '"a-b-a"', '"$"', '"10"', '"9"', '"-0"', '"0"', '"\\t x \\n"', '"ab$&c"', '"aaa"'];
    const R = ['"$&"', '"$$"', '"$\'"', '"$`"', '"$1"', '"x"', '""', '"$0"', '"$<n>"'];
    const N = ['-0', '0', '0.5', '-0.5', '1.5', '-1.5', '2.5', '-2.5', '1e21', '-1e21', '1e-7', '255', '-255', 'NaN', 'Infinity', '-Infinity', '2 ** 53', '2 ** 31', '0.1 + 0.2', '123.456', '5e-324', '1 / 3'];
    const a = () => this.pick(A);
    const af = () => this.pick(Afin);
    const s = () => this.pick(S);
    const n = () => this.pick(N);
    const T9 = [
      () => `${s()}.charAt(${a()})`,
      () => `${s()}.charCodeAt(${a()})`,
      () => `${s()}.at(${a()})`,
      () => `${s()}.startsWith(${s()}, ${a()})`,
      () => `${s()}.endsWith(${s()}${this.chance(0.5) ? `, ${a()}` : ''})`,
      () => `${s()}.${this.pick(['replace', 'replaceAll'])}(${s()}, ${this.pick(R)})`,
      () => `${s()}.${this.pick(['toLowerCase', 'toUpperCase', 'trim', 'trimStart', 'trimEnd'])}()`,
      () => `${s()}.concat(${a()}, ${a()})`,
      () => `${s()}.${this.pick(['substring', 'slice'])}(${af()}${this.chance(0.6) ? `, ${af()}` : ''})`,
      // posiciones fuera de rango en indexOf/lastIndexOf: G, oxc#25158 (solo con --con-conocidos). Sin el,
      // indexOf no recibe una posicion mayor que el largo de su receptor (con S, solo `1` sobre `""`:
      // `"".indexOf("", 1)` se pliega a 1, node da 0). Un solo pick, como antes.
      () => {
        const rec = s();
        const m = this.pick(['indexOf', 'lastIndexOf']);
        const largo = m === 'indexOf' ? JSON.parse(rec).length : Infinity;
        return `${rec}.${m}(${s()}${this.chance(0.5) ? `, ${CONOCIDOS ? af() : this.pick(['0', '1', '-1', '-0', '0.5'].filter((p) => Math.trunc(Number(p)) <= largo))}` : ''})`;
      },
      () => `(${n()}).toString(${this.pick(['', '2', '16', '36', '10', '8', '3', 'undefined'])})`,
      () => `Math.${this.pick(['round', 'ceil', 'floor', 'trunc', 'sign', 'abs', 'fround', 'clz32', 'sqrt', ...(CONOCIDOS ? ['cbrt'] : [])])}(${this.chance(0.7) ? n() : a()})`,
      () => `Math.${this.pick(['min', 'max', 'imul'])}(${[...Array(this.int(4))].map(() => (this.chance(0.7) ? n() : a())).join(', ')})`,
      () => `Math.pow(${n()}, ${n()})`,
      () => `(${n()}) ** (${n()})`,
      () => `String.fromCharCode(${[...Array(1 + this.int(3))].map(() => this.pick(['65', '65.9', '-1', '65601', '"66"', 'undefined', 'NaN', '0x10FFFF', '2 ** 32 + 67', 'null', 'true'])).join(', ')})`,
      // isInteger/isSafeInteger de un diminuto: H, oxc#27622 (solo con --con-conocidos)
      () => { const f = this.pick(['Number.isInteger', 'Number.isSafeInteger', 'Number.isNaN', 'Number.isFinite', 'isNaN', 'isFinite']); let x = a(); if (!CONOCIDOS && /Integer/.test(f) && x === '5e-324') x = '0.5'; return `${f}(${x})`; },
      () => `Array.of(${a()}, ${a()}).join()`,
      () => `[${a()}, ${a()}].concat(${a()}, [${a()}]).join("|")`,
      () => `\`\${${a()}}|\${${n()}}|\${[${a()}, [${a()}]]}\``,
      () => `${this.pick([s(), `[${a()}, , ${a()}]`, '[,,]', '[1, [2, 3]]'])}.length`,
      () => `${this.pick(['/a\\/b/', '/[/]/g', '/\\d+/i', 'new RegExp("a/b")'])}.source`,
      () => `${this.pick([s(), `[${a()}, ${a()}, ${a()}]`])}[${this.pick(['0', '1', '-1', '1.5', '"1"', '"01"', '"length"', '2 ** 32', '-0', '"-0"', '3'])}]`,
      () => `${s()} + ${this.pick([n(), a()])}`,
      () => `${this.pick([n(), a()])} + ${s()} + ${this.pick([n(), a()])}`,
      () => `(${this.pick([n(), a()])}) ${this.pick(['|', '&', '^', '<<', '>>', '>>>'])} (${this.pick([n(), a()])})`,
      () => `${this.pick(['~', '-', '+', '!', '~~'])}(${this.pick([n(), a()])})`,
      () => `(${a()}) ${this.pick(['<', '>', '<=', '>=', '==', '!=', '===', '!=='])} (${a()})`,
      () => `${this.pick(['Number', 'parseInt', 'parseFloat', 'String', 'Boolean'])}(${this.pick(['" 0x10 "', '"0b11"', '"0o7"', ...(CONOCIDOS ? ['"1_000"', '"\\u00857"'] : []), '""', '" "', '"-0"', '"1e3"', '".5e1"', '"Infinity"', '"-Infinity"', '"12px"', '"0x"', '"+5"', '"1e1000"', '"\\u00a0 7"', a()])})`,
      () => `parseInt(${this.pick(['"11"', '"zz"', '"0x1f"', '"-0"', '" 9 "', '"1e3"', '0.0000005', '1e21', '-0'])}, ${this.pick(['2', '16', '36', '0', '1', '37', 'undefined', '-1', '10'])})`,
      () => `typeof (${a()}) ${this.pick(['===', '!=='])} "${this.pick(['number', 'object', 'undefined', 'string', 'boolean'])}"`,
      () => `(${a()}) ?? (${a()})`,
      () => `${this.pick(['1n', '0n', '(-1n)', '(2n ** 64n)', 'BigInt(-0)', 'BigInt("0x10")'])} ${this.pick(['+', '-', '*', '/', '%', '**', '&', '|', '^', '<<', '>>'])} ${this.pick(['1n', '3n', '-2n', '64n'])}`,
      // BigInt no evaluable (2n ** 64n) comparado con laxitud: K, oxc#27656 (solo con --con-conocidos)
      () => `${this.pick(['1n', '(-0n)', ...(CONOCIDOS ? ['2n ** 64n'] : []), '(-1n)'])} ${this.pick(['==', '<', '>=', '==='])} ${this.pick(['1', '-0', '"1"', '2 ** 64', 'true', '"x"', 'NaN', 'Infinity'])}`,
      () => `void (${a()})`,
      // URI: url_encoding de oxc_ecmascript. Pueden lanzar URIError: van en try (ver abajo)
      () => `${this.pick(['encodeURI', 'encodeURIComponent', 'decodeURI', 'decodeURIComponent'])}(${this.pick(['"%"', '"%E0%A4%A"', '"%C3%A9"', '"%c3%a9"', '";/?:@&=+$,#"', '"\\u00e9"', '"a b"', '"%25"', '"%2F%3f%23"', '"%ED%A0%80"', '"%F0%9F%98%80"', '"%C0%80"', '"%FF"', '"\\u{1F600}"', '"-_.!~*()\'"', '"%41%42"', '"%7e"', '"\\u0000"', '"%00"', '"%e2%82%ac"', ...(CONOCIDOS ? ['"\\ud800"'] : [])])})`,
      () => `(${a()}, ${a()})`,
      () => `!!(${a()}) ${this.pick(['&&', '||'])} ${a()}`,
    ];
    const out = ['const r = [];'];
    const k = 8 + this.int(10);
    for (let i = 0; i < k; i++) out.push(`r.push(${this.pick(T9)()});`);
    // BigInt / y division por cero de BigInt lanzan: aislado en try por expresion
    out.push('return r;');
    return out.map((l) => l.replace(/^r\.push\((.*(?:n [\/%] |n \*\* -|\*\* -2n|>> -2n|<< -2n|URI).*)\);$/, 'try { r.push($1); } catch (e) { r.push("E:" + e.constructor.name); }'));
  }

  // F10. Cantidad y orden de evaluaciones de un mismo blanco (minimize_conditions
  // combined assignment, minimize_logical_expression a ||=, x == null, typeof ... && !== null,
  // substitute_iife_call, merge_assignment_to_declaration, minimize_for_statement): el
  // blanco es un getter o una clave computada con efecto, y la traza T cuenta las lecturas.
  F10(sc) {
    const o = sc.objs.length ? this.pick(sc.objs) : null;
    const out = [];
    if (!o) return [`return t("sin-objeto", 0);`];
    const key = () => this.pick([`t("k", "p")`, `t("k", "q")`, `"p"`, `t("k", "s")`]);
    const tgt = () => this.pick([`${o}.p`, `${o}.q`, `${o}[${key()}]`, `${o}.s`]);
    const v = () => this.expr(sc, 1);
    const n = 2 + this.int(4);
    for (let i = 0; i < n; i++) {
      const k = this.int(16);
      const x = tgt();
      if (k === 0) out.push(`${x} = ${x} ${this.pick(['+', '-', '|', '*', '**', '??', '||', '&&'])} ${v()};`);
      else if (k === 1) out.push(`${x} || (${x} = ${v()});`);
      else if (k === 2) out.push(`${x} ?? (${x} = ${v()});`);
      else if (k === 3) out.push(`${x} && (${x} = ${v()});`);
      else if (k === 4) out.push(`P("nl", ${x} === null || ${x} === void 0, ${x} !== null && ${x} !== undefined);`);
      else if (k === 5) out.push(`P("io", typeof ${x} == "object" && ${x} !== null, typeof ${x} != "object" || ${x} === null);`);
      else if (k === 6) out.push(`if (${x} == null) ${x} = ${v()};`);
      else if (k === 7) out.push(`if (!${x}) ${x} = ${v()}; else t("el", 0);`);
      else if (k === 8) out.push(`P("ii", (function (x1, y1) { return [y1, x1]; })(${x}, ${tgt()}));`);
      else if (k === 9) { const z = this.id('z'); out.push(`let ${z}; ${z} = ${x}; P("md", ${z});`); }
      else if (k === 10) { const z = this.id('z'); out.push(`let ${z} = 0; for (;;) { if (${z}++ > 1 || ${x}) break; t("fb", ${z}); }`); }
      else if (k === 11) out.push(`${x}++; ${x} += 1; ${this.pick(['--', '++'])}${x};`);
      else if (k === 12) out.push(`P("cq", ${x} ? ${x} : ${v()}, ${x} ? ${v()} : ${x});`);
      else if (k === 13) out.push(`P("ne", !(${x} != null), !!${x} === true);`);
      else if (k === 14) out.push(`[${x}, ${tgt()}] = [${tgt()}, ${x}];`);
      else out.push(`P("cc", (${x}, ${x}), void ${x}, typeof ${x});`);
    }
    out.push(`return [${tgt()}, T.length];`);
    return out;
  }

  programa() {
    const lines = [
      '"use strict";',
      'const T = [];',
      'function t(k, v) { T.push(k); return v; }',
      'function S(v, d) {',
      '  if (typeof v === "function") return "fn";',
      '  if (typeof v === "symbol") return "sym";',
      '  if (typeof v === "bigint") return v + "n";',
      '  if (v instanceof Error) return "E:" + v.constructor.name;',
      '  if (Array.isArray(v)) return d > 3 ? "[..]" : "[" + v.map((x) => S(x, d + 1)).join(",") + "]";',
      '  if (v !== null && typeof v === "object") {',
      '    if (d > 3) return "{..}";',
      '    return (Object.getPrototypeOf(v) === null ? "null" : "") + "{" + Reflect.ownKeys(v).map((k) => {',
      '      const ds = Object.getOwnPropertyDescriptor(v, k);',
      '      return S(k, d + 1) + ":" + ("value" in ds ? S(ds.value, d + 1) : "acc");',
      '    }).join(",") + "}";',
      '  }',
      '  if (Object.is(v, -0)) return "-0";',
      '  return typeof v === "string" ? JSON.stringify(v) : String(v);',
      '}',
      'function P(...a) { console.log(a.map((x) => S(x, 0)).join(" ")); }',
    ];
    const ns = 2 + this.int(4);
    for (let i = 0; i < ns; i++) {
      const fam = this.pick(this.familias);
      lines.push(`// ${fam}`);
      lines.push(...this.seccion(i, fam));
    }
    lines.push('P("fin", T.length);');
    return lines.join('\n') + '\n';
  }
}

function main() {
  const argv = process.argv.slice(2);
  if (argv.includes('-h') || argv.includes('--help')) {
    // La ayuda es la cabecera de este archivo (los comentarios `//` desde la linea 3).
    const lineas = fs.readFileSync(__filename, 'utf8').split('\n').slice(2);
    const fin = lineas.findIndex((l) => !l.startsWith('//'));
    process.stdout.write(lineas.slice(0, fin).map((l) => l.replace(/^\/\/ ?/, '')).join('\n') + '\n');
    process.exit(0);
  }
  if (argv.length < 2) {
    process.stderr.write('uso: generar.cjs DIR_SALIDA CANTIDAD [--semilla N] [--familias F1,F2,...] [--desde K] [--con-conocidos] (ver --help)\n');
    process.exit(2);
  }
  // Validacion: un valor mal escrito no puede dar en silencio otra tanda (Number('x') es NaN y
  // `NaN | 0` es 0; Number('') es 0). Solo valida: los valores aceptados generan lo mismo que antes.
  const num = (s) => (typeof s === 'string' && /^-?[0-9]+$/.test(s) ? Number(s) : NaN);
  const out = argv[0];
  const cant = num(argv[1]);
  let semilla = 1;
  let desde = 0;
  let familias = ['F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10'];
  for (let i = 2; i < argv.length; i++) {
    if (argv[i] === '--semilla') semilla = num(argv[++i]);
    else if (argv[i] === '--familias') familias = (argv[++i] || '').split(',');
    else if (argv[i] === '--desde') desde = num(argv[++i]);
    else if (argv[i] === '--con-conocidos') {}
    else { process.stderr.write(`opcion desconocida: ${argv[i]}\n`); process.exit(2); }
  }
  const malo = (v) => !Number.isSafeInteger(v);
  if (malo(cant) || cant < 0 || malo(semilla) || malo(desde) || desde < 0) {
    process.stderr.write('CANTIDAD, --semilla y --desde tienen que ser enteros (CANTIDAD y --desde >= 0)\n');
    process.exit(2);
  }
  const desconocidas = familias.filter((f) => !/^F([1-9]|10)$/.test(f));
  if (!familias.length || desconocidas.length) {
    process.stderr.write(`familias desconocidas: ${desconocidas.join(',') || '(ninguna dada)'}; validas: F1..F10\n`);
    process.exit(2);
  }
  for (let i = desde; i < desde + cant; i++) {
    const g = new Gen((semilla * 1000003 + i) | 0, familias);
    const dir = path.join(out, String(i).padStart(5, '0'));
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'input.js'), g.programa());
  }
}

if (require.main === module) main();
module.exports = { Gen };
