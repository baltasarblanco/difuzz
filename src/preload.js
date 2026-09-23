'use strict';
// difuzz preload. Se carga con `node --require` antes de A y antes de B, igual en los dos.
// No pasa por el minifier: es infraestructura del oraculo, no parte del programa.
//
// Tres trabajos:
//   1. Determinismo: Date, performance.now y Math.random dan lo mismo en cada ejecucion.
//   2. Observador: define __difuzzObserve, que el epilogo llama con los bindings top-level
//      de programas que no imprimen nada.
//   3. Excepciones: un throw no atrapado imprime su tipo y sale con 70 (runner::THROW_EXIT).
//   4. Sensor de asunciones: marca si el programa usa algo que AGENTS.md del minifier
//      declara asumido (toString de funciones, .stack, Proxy, built-ins pisados, TDZ).
//      Al salir escribe los flags en stderr (runner::FLAGS_TAG), que no se compara.
//
// Las lineas que escribe difuzz (no el programa) empiezan con "!dz " (runner::DZ).
(() => {
  const FIXED_TIME = 1700000000000;

  const RealDate = Date;
  function FixedDate(...args) {
    if (!new.target) return new RealDate(FIXED_TIME).toString();
    return Reflect.construct(RealDate, args.length ? args : [FIXED_TIME], new.target);
  }
  FixedDate.prototype = RealDate.prototype;
  FixedDate.now = () => FIXED_TIME;
  FixedDate.parse = RealDate.parse;
  FixedDate.UTC = RealDate.UTC;
  Object.defineProperty(FixedDate, 'name', { value: 'Date' });
  Object.defineProperty(FixedDate, 'length', { value: 7 });
  globalThis.Date = FixedDate;

  try {
    globalThis.performance.now = () => 0;
  } catch {}

  // mulberry32 con semilla fija: cada proceso de node arranca la misma secuencia.
  let seed = 0x2f6b8a1d | 0;
  Math.random = function random() {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = seed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };

  // Se captura antes de que el programa pueda pisar console.log o process.stdout.write.
  const write = process.stdout.write.bind(process.stdout);
  const writeErr = process.stderr.write.bind(process.stderr);

  // --- Sensor de asunciones ---------------------------------------------------
  const flags = new Set();

  // Node usa primordials internos: console.log(f) no pasa por aca, solo el programa.
  const realToString = Function.prototype.toString;
  Object.defineProperty(Function.prototype, 'toString', {
    value: {
      toString() {
        flags.add('fn_tostring');
        return realToString.call(this);
      },
    }.toString,
    writable: true,
    configurable: true,
    enumerable: false,
  });

  // V8 llama a prepareStackTrace la primera vez que alguien lee .stack.
  Error.prepareStackTrace = (err, frames) => {
    flags.add('stack');
    let head;
    try {
      head = String(err);
    } catch {
      head = 'Error';
    }
    return head + frames.map((f) => `\n    at ${f}`).join('');
  };

  const RealProxy = Proxy;
  globalThis.Proxy = new RealProxy(RealProxy, {
    construct(target, args) {
      flags.add('proxy');
      return new target(...args);
    },
    get(target, key) {
      if (key === 'revocable') {
        return (...args) => {
          flags.add('proxy');
          return target.revocable(...args);
        };
      }
      return Reflect.get(target, key);
    },
  });
  const MAX_DEPTH = 4;
  const MAX_ITEMS = 20;

  // Serializador determinista. Nunca invoca getters (lee descriptores) y nunca imprime
  // codigo fuente ni nombres de funciones: esos cambian legitimamente al minificar.
  const ser = (v, depth, ancestors) => {
    switch (typeof v) {
      case 'undefined': return 'undefined';
      case 'number': return Object.is(v, -0) ? '-0' : String(v);
      case 'bigint': return `${v}n`;
      case 'string': return JSON.stringify(v);
      case 'boolean': return String(v);
      case 'symbol': return `Symbol(${JSON.stringify(v.description ?? '')})`;
      case 'function': return 'fn';
    }
    if (v === null) return 'null';
    if (ancestors.has(v)) return '[cycle]';
    if (depth >= MAX_DEPTH) return '[...]';
    ancestors.add(v);
    try {
      if (v instanceof RealDate) return `Date(${RealDate.prototype.getTime.call(v)})`;
      if (v instanceof RegExp) return String(v);
      if (v instanceof Error) return `${v.name}: ${v.message}`;
      if (v instanceof Map) {
        const out = [];
        for (const [k, val] of v) {
          if (out.length === MAX_ITEMS) { out.push(`...${v.size}`); break; }
          out.push(`${ser(k, depth + 1, ancestors)} => ${ser(val, depth + 1, ancestors)}`);
        }
        return `Map{${out.join(', ')}}`;
      }
      if (v instanceof Set) {
        const out = [];
        for (const val of v) {
          if (out.length === MAX_ITEMS) { out.push(`...${v.size}`); break; }
          out.push(ser(val, depth + 1, ancestors));
        }
        return `Set{${out.join(', ')}}`;
      }
      const field = (k) => {
        const d = Object.getOwnPropertyDescriptor(v, k);
        if (!d) return '<hole>';
        return 'value' in d ? ser(d.value, depth + 1, ancestors) : '[accessor]';
      };
      if (Array.isArray(v)) {
        const n = Math.min(v.length, MAX_ITEMS);
        const out = [];
        for (let i = 0; i < n; i++) out.push(field(String(i)));
        if (v.length > MAX_ITEMS) out.push(`...${v.length}`);
        return `[${out.join(', ')}]`;
      }
      const keys = Object.keys(v);
      const out = keys.slice(0, MAX_ITEMS).map((k) => `${JSON.stringify(k)}: ${field(k)}`);
      if (keys.length > MAX_ITEMS) out.push(`...${keys.length}`);
      return `{${out.join(', ')}}`;
    } finally {
      ancestors.delete(v);
    }
  };

  // Oraculo de excepciones. Solo el tipo: el mensaje trae nombres mangleados
  // ("n is not a function"). Clases propias que extienden Error se ven como su
  // built-in mas cercano, porque su nombre tambien puede cambiar.
  const ERRORS = [AggregateError, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError, Error];
  const thrownType = (e) => {
    if (e === null || (typeof e !== 'object' && typeof e !== 'function')) {
      return `valor ${ser(e, 0, new Set())}`;
    }
    for (const E of ERRORS) {
      try {
        if (e instanceof E) return E.name;
      } catch {}
    }
    return 'objeto';
  };
  // process.exit y no exitCode: despues de un uncaughtException, node seguiria
  // corriendo timers pendientes, y un crash real no los corre.
  const onThrow = (e) => {
    try {
      if (e instanceof ReferenceError && / before initialization/.test(e.message)) flags.add('tdz');
    } catch {}
    write(`!dz throw ${thrownType(e)}\n`);
    process.exit(70);
  };
  process.on('uncaughtException', onThrow);
  process.on('unhandledRejection', onThrow);

  Object.defineProperty(globalThis, '__difuzzObserve', {
    value: (...pairs) => {
      for (const [name, value] of pairs) {
        let s;
        try {
          s = ser(value, 0, new Set());
        } catch (e) {
          s = `[throws ${e && e.name}]`;
        }
        write(`${name} = ${s}\n`);
      }
    },
    enumerable: false,
  });

  // Built-ins pisados: foto de los descriptores al final del preload (despues de
  // lo que el preload mismo cambia) contra los descriptores al salir.
  const WATCHED = [
    Object, Function, Array, String, Number, Boolean, Symbol, BigInt, RegExp, Promise,
    Map, Set, WeakMap, WeakSet, Error, globalThis.Date, Math, JSON, Reflect,
  ];
  const GLOBAL_NAMES = [
    'undefined', 'NaN', 'Infinity', 'eval', 'isNaN', 'isFinite', 'parseInt', 'parseFloat',
    'Object', 'Function', 'Array', 'String', 'Number', 'Boolean', 'Symbol', 'BigInt',
    'RegExp', 'Promise', 'Map', 'Set', 'WeakMap', 'WeakSet', 'Error', 'Date', 'Math',
    'JSON', 'Reflect', 'Proxy',
  ];
  const describe = (o, k) => {
    const d = Object.getOwnPropertyDescriptor(o, k);
    return d ? [d.value, d.get, d.set] : null;
  };
  const photo = () => {
    const shots = [];
    for (const o of WATCHED) {
      for (const target of [o, o.prototype]) {
        if (target === null || (typeof target !== 'object' && typeof target !== 'function')) continue;
        for (const k of Reflect.ownKeys(target)) shots.push([target, k, describe(target, k)]);
        shots.push([target, null, Reflect.ownKeys(target).length]);
      }
    }
    for (const k of GLOBAL_NAMES) shots.push([globalThis, k, describe(globalThis, k)]);
    return shots;
  };
  const before = photo();
  const builtinsChanged = () => {
    for (const [o, k, shot] of before) {
      if (k === null) {
        if (Reflect.ownKeys(o).length !== shot) return true;
        continue;
      }
      const now = describe(o, k);
      if ((now === null) !== (shot === null)) return true;
      if (now && (now[0] !== shot[0] || now[1] !== shot[1] || now[2] !== shot[2])) {
        // NaN !== NaN: el global NaN no cambio si sigue siendo NaN.
        if (!(Number.isNaN(now[0]) && Number.isNaN(shot[0]))) return true;
      }
    }
    return false;
  };

  process.on('exit', () => {
    try {
      if (builtinsChanged()) flags.add('builtins');
    } catch {
      flags.add('builtins');
    }
    writeErr(`\n__DIFUZZ_FLAGS__ ${[...flags].sort().join(',')}\n`);
  });
})();
