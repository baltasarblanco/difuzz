'use strict';
// difuzz preload. Se carga con `node --require` antes de A y antes de B, igual en los dos.
// No pasa por el minifier: es infraestructura del oraculo, no parte del programa.
//
// Cinco trabajos:
//   1. Determinismo: Date, performance.now y Math.random dan lo mismo en cada ejecucion.
//   2. Observador: define __difuzzObserve, que el epilogo llama con los bindings top-level
//      de programas que no imprimen nada.
//   3. Excepciones: un throw no atrapado imprime su tipo y sale con 70 (runner::THROW_EXIT).
//   4. Sensor de asunciones: marca si el programa usa algo que AGENTS.md del minifier
//      declara asumido (toString de funciones, .stack, Proxy, built-ins pisados, TDZ).
//      Al salir escribe los flags por el canal de control (M10, ver abajo).
//   5. Canales: console.error/warn/trace van a stdout (stderr no se compara), y al salir
//      se imprimen los globals que el programa creo (sin tocar el fuente: sin efecto
//      observador).
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

  // M20: lo demas que cambia entre ejecuciones, fijo (mismo patron que Date). Valores
  // inventados pero estables: A y B ven lo mismo, y una corrida ve lo mismo que otra.
  const fix = (obj, key, value) => {
    try {
      Object.defineProperty(obj, key, { value, writable: true, configurable: true, enumerable: true });
    } catch {}
  };
  fix(process, 'pid', 4242);
  fix(process, 'ppid', 4241);
  // hrtime avanza 1 ms por llamada (un programa que mide un intervalo ve algo > 0).
  let hrNow = 0n;
  const HR_STEP = 1000000n;
  const hrtime = (prev) => {
    hrNow += HR_STEP;
    let s = Number(hrNow / 1000000000n);
    let ns = Number(hrNow % 1000000000n);
    if (Array.isArray(prev)) {
      s -= prev[0];
      ns -= prev[1];
      if (ns < 0) {
        s -= 1;
        ns += 1e9;
      }
    }
    return [s, ns];
  };
  hrtime.bigint = () => {
    hrNow += HR_STEP;
    return hrNow;
  };
  fix(process, 'hrtime', hrtime);
  fix(process, 'uptime', () => 1);
  const MEM = { rss: 50000000, heapTotal: 20000000, heapUsed: 10000000, external: 1000000, arrayBuffers: 10000 };
  const memoryUsage = () => ({ ...MEM });
  memoryUsage.rss = () => MEM.rss;
  fix(process, 'memoryUsage', memoryUsage);
  try {
    const os = require('os');
    fix(os, 'freemem', () => 1073741824);
    fix(os, 'uptime', () => 1);
    fix(os, 'loadavg', () => [0, 0, 0]);
  } catch {}
  // crypto: PRNG sembrado (mulberry32, otra semilla que Math.random).
  let cseed = 0x1b873593 | 0;
  const rnd8 = () => {
    cseed = (cseed + 0x6d2b79f5) | 0;
    let t = cseed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) & 0xff;
  };
  const fillRandom = (view) => {
    const u8 = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
    for (let i = 0; i < u8.length; i++) u8[i] = rnd8();
    return view;
  };
  const randomUUID = () => {
    const b = fillRandom(new Uint8Array(16));
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
  };
  try {
    const nodeCrypto = require('crypto');
    fix(nodeCrypto, 'randomUUID', randomUUID);
    fix(nodeCrypto, 'getRandomValues', fillRandom);
    fix(nodeCrypto, 'randomFillSync', (buf) => fillRandom(buf));
    fix(nodeCrypto, 'randomBytes', (n, cb) => {
      const b = fillRandom(Buffer.alloc(n));
      if (typeof cb === 'function') {
        process.nextTick(cb, null, b);
        return undefined;
      }
      return b;
    });
    fix(nodeCrypto, 'randomInt', (min, max, cb) => {
      if (max === undefined || typeof max === 'function') {
        cb = max;
        max = min;
        min = 0;
      }
      const v = min + ((rnd8() << 16) | (rnd8() << 8) | rnd8()) % (max - min);
      if (typeof cb === 'function') {
        process.nextTick(cb, null, v);
        return undefined;
      }
      return v;
    });
  } catch {}
  try {
    fix(globalThis.crypto, 'randomUUID', randomUUID);
    fix(globalThis.crypto, 'getRandomValues', fillRandom);
  } catch {}

  // Se captura antes de que el programa pueda pisar console.log o process.stdout.write.
  const write = process.stdout.write.bind(process.stdout);

  // M10: canal de control propio (archivo DIFUZZ_CTL que el runner trunca antes de
  // cada ejecucion), fuera de stdout/stderr/exit code, que el programa tambien puede
  // escribir. Lineas: `throw <Tipo>`, `link <code>`, `flags a,b`, `dz` (una por cada
  // linea "!dz " que escribe el preload en stdout) y `global` (una por global creado).
  // process.exit se captura aca: un programa que lo pisa no evita que un throw termine.
  const fs = require('fs');
  const realExit = process.exit.bind(process);
  let ctlFd = null;
  try {
    if (process.env.DIFUZZ_CTL) ctlFd = fs.openSync(process.env.DIFUZZ_CTL, 'a');
  } catch {}
  delete process.env.DIFUZZ_CTL;
  const ctl = (line) => {
    if (ctlFd === null) return;
    try {
      fs.writeSync(ctlFd, `${line}\n`);
    } catch {}
  };
  // Una linea "!dz " del preload: se cuenta en el canal (runner::Exec::own_output).
  const writeDz = (s) => {
    ctl('dz');
    write(s);
  };

  // --- Sensor de asunciones ---------------------------------------------------
  const flags = new Set();
  // Corrida "salada" (runner::run_salted): mismo programa, otro contenido de
  // toString y .stack. Se borra del entorno para que el programa no la vea.
  const SALT = process.env.DIFUZZ_SALT === '1';
  // t-principio: contenido FIJO (re-chequeo de contenido, recheck::contenido_matters): el mismo
  // toString y el mismo .stack para cualquier codigo, asi A' y B' no dependen del texto.
  const FIJO = process.env.DIFUZZ_SALT === 'fijo';
  delete process.env.DIFUZZ_SALT;

  // Node usa primordials internos: console.log(f) no pasa por aca, solo el programa.
  const realToString = Function.prototype.toString;
  Object.defineProperty(Function.prototype, 'toString', {
    value: {
      toString() {
        flags.add('fn_tostring');
        const s = realToString.call(this);
        if (FIJO) return 'function () { [native code] }';
        return SALT ? `${s}/*dz*/` : s;
      },
    }.toString,
    writable: true,
    configurable: true,
    enumerable: false,
  });

  // V8 llama a prepareStackTrace la primera vez que alguien lee .stack. `quiet`: el
  // preload mismo lee el stack de un throw (M21) y eso no es uso del programa.
  let quiet = false;
  // b4-a: `marcos`, los frames de cada error formateado, para ubicar una violacion no
  // atrapada (onThrow). Ojo: el sensor de violaciones atrapadas (al final) hace que V8
  // formatee el stack de cada Error atrapado al pausar, y eso prende `stack` aunque el
  // programa no lo lea; no se descuenta a proposito (despues del formateo, una lectura del
  // programa ya no pasa por aca): el re-chequeo de contenido (A salado) decide.
  const marcos = new WeakMap();
  Error.prepareStackTrace = (err, frames) => {
    if (!quiet) flags.add('stack');
    let head;
    try {
      head = String(err);
    } catch {
      head = 'Error';
    }
    try {
      if (frames.length && err !== null && (typeof err === 'object' || typeof err === 'function')) marcos.set(err, frames);
    } catch {}
    // t-principio: con contenido fijo, la cabecera y una linea constante por marco (sin
    // posiciones ni nombres): A' conserva las lineas de A y `localizada` puede compararlas.
    if (FIJO) return head + frames.map(() => '\n    at dz').join('');
    return head + frames.map((f) => `\n    at ${f}`).join('') + (SALT ? '\n    at dz' : '');
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
  // (Definido antes de `ser`, que lo usa para los Error: M6.)

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
      // M6: ni name ni message (traen nombres locales que el minifier renombra,
      // y una subclase propia tambien cambia de nombre): solo el tipo, como un throw.
      if (v instanceof Error) return `Error(${thrownType(v)})`;
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
      // Prototipo: una instancia y un objeto plano con las mismas claves no son lo mismo.
      // Claves propias todas (symbol y no enumerables con ~): mangle.props esta apagado,
      // asi que los nombres de propiedades no cambian al minificar.
      const proto = Object.getPrototypeOf(v);
      const tag = proto === Object.prototype ? '' : proto === null ? 'null' : 'inst';
      const keys = Reflect.ownKeys(v);
      const out = keys.slice(0, MAX_ITEMS).map((k) => {
        const d = Object.getOwnPropertyDescriptor(v, k);
        const name = typeof k === 'symbol' ? `[${String(k)}]` : JSON.stringify(k);
        return `${d && !d.enumerable ? '~' : ''}${name}: ${field(k)}`;
      });
      if (keys.length > MAX_ITEMS) out.push(`...${keys.length}`);
      return `${tag}{${out.join(', ')}}`;
    } finally {
      ancestors.delete(v);
    }
  };

  // process.exit y no exitCode: despues de un uncaughtException, node seguiria
  // corriendo timers pendientes, y un crash real no los corre.
  // M13: "Class extends value X is not a constructor or null" (asuncion de oxc: extender
  // una clase no tiene efectos).
  const extendsFlag = (e) => {
    try {
      if (e instanceof TypeError && /^Class extends value/.test(e.message)) flags.add('extends');
    } catch {}
  };
  // b4-a: violaciones de asunciones de oxc que V8 informa con un error propio: TDZ, la
  // coercion de un Symbol a primitivo y un `class extends` con algo que no es constructor.
  const VIOLACIONES = [
    ['tdz', ReferenceError, / before initialization/],
    ['symbol', TypeError, /^Cannot convert a Symbol value to a/],
    ['extends', TypeError, /^Class extends value/],
  ];
  const violacionDe = (e) => {
    for (const [k, E, re] of VIOLACIONES) {
      try {
        if (e instanceof E && re.test(e.message)) return k;
      } catch {}
    }
    return null;
  };
  // `viol <clase> <linea>:<columna>` (0-based, columna en UTF-16) o `viol <clase> -`. Las
  // atrapadas las registra el sensor del inspector; la de Symbol no atrapada, onThrow.
  const MAX_VIOL = 32;
  // Apaga (false) y vuelve a prender (true) el sensor del inspector mientras el preload
  // atrapa errores que no son del programa (el hook de --stubs): al pausar, V8 formatea su
  // stack y prenderia `stack`. Lo define el sensor; sin inspector no hace nada.
  let sensor = () => {};
  // I3 (revision-b456): el sensor llego a MAX_PAUSAS y se apago a mitad de la corrida
  // (linea `sensor_tope` del canal, al salir): el resto corrio sin sus efectos.
  let sensorTope = false;
  const violaciones = [];
  // `desc`: la descripcion de V8 (el stack formateado), para reconocer despues el mismo error.
  // `noAtrapada` (I2, revision-b456): la TDZ que nadie del programa atrapo; ya tiene su flag
  // (`tdz`), asi que al salir solo deja su linea `viol` (para el re-chequeo) y no `tdz_atrapada`.
  const registrarViolacion = (k, pos, desc, noAtrapada) => {
    if (violaciones.length < MAX_VIOL) violaciones.push([k, pos, desc, noAtrapada === true]);
  };
  const posDeMarco = (e) => {
    try {
      // El primero con archivo: `Math.abs (<anonymous>)` es un built-in, sin posicion.
      const f = (marcos.get(e) || []).find((x) => x.getFileName());
      const l = f && f.getLineNumber();
      const c = f && f.getColumnNumber();
      if (Number.isInteger(l) && Number.isInteger(c)) return `${l - 1}:${c - 1}`;
    } catch {}
    return '-';
  };
  // I2: la posicion de un error que el programa no atrapo. Lee `.stack` en silencio (no es
  // uso del programa) para que V8 llame a prepareStackTrace y queden sus `marcos`.
  const posDeThrow = (e) => {
    try {
      quiet = true;
      void e.stack;
    } catch {
    } finally {
      quiet = false;
    }
    return posDeMarco(e);
  };
  const onThrow = (e) => {
    try {
      if (e instanceof ReferenceError && / before initialization/.test(e.message)) flags.add('tdz');
    } catch {}
    extendsFlag(e);
    const t = thrownType(e);
    ctl(`throw ${t}`);
    // M21: mensaje y stack por el canal (no se comparan; van a a.exc/b.exc del hallazgo).
    try {
      quiet = true;
      const text = e !== null && typeof e === 'object' ? String(e.stack || e.message || e) : String(e);
      ctl(`exc ${text.replace(/\\/g, '\\\\').replace(/\n/g, '\\n')}`);
    } catch {
    } finally {
      quiet = false;
    }
    // b4-a: V8 puede haber pausado por este mismo error como "atrapado" (en el carril B, el
    // rechazo del import lo maneja node): no lo fue. Se reconoce por su stack formateado.
    const k = violacionDe(e);
    if (k) {
      try {
        quiet = true;
        const st = e.stack;
        const i = violaciones.findIndex((v) => v[0] === k && v[2] === st);
        if (i >= 0) violaciones.splice(i, 1);
      } catch {
      } finally {
        quiet = false;
      }
    }
    // La coercion de un Symbol no tenia flag ni atrapada ni sin atrapar.
    if (k === 'symbol') registrarViolacion('symbol', posDeMarco(e));
    // I2 (revision-b456, r09-tdz-final-getter): la TDZ no atrapada tambien se ubica, para
    // que `tdz` pase por el re-chequeo de violaciones (antes iba directo a known/).
    if (k === 'tdz') registrarViolacion('tdz', posDeThrow(e), null, true);
    writeDz(`!dz throw ${t}\n`);
    realExit(70);
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
        if (String(name).startsWith('!dz ')) ctl('dz');
        write(`${name} = ${s}\n`);
      }
    },
    enumerable: false,
  });

  // stderr no se compara: lo que el programa escribe por console.error/warn/trace pasaria
  // desapercibido (y un programa que solo usa esos contaria como mudo). trace sin stack.
  const util = require('util');
  for (const m of ['error', 'warn', 'trace']) {
    console[m] = (...args) => {
      write(`[${m}] ${util.format(...args)}\n`);
    };
  }

  // M30 / re-chequeo de `global_conocido`: stub inerte y determinista. Una funcion
  // (ser la imprime `fn`) cuyas propiedades, llamadas y `new` devuelven otros stubs
  // (cacheados por camino: `s.a === s.a`). `then` y `__esModule` son undefined (no es
  // thenable ni ESM de Babel), el primitivo es 0 o "[stub <camino>]" y se itera vacio.
  // Lo que el programa le asigna se lee de vuelta. RealProxy: no prende `proxy`.
  const stubs = new Map();
  const stub = (path) => {
    const hit = stubs.get(path);
    if (hit) return hit;
    const target = [function () {}][0];
    const p = new RealProxy(target, {
      get(t, k) {
        if (k === 'then' || k === '__esModule') return undefined;
        if (k === Symbol.toPrimitive) return (hint) => (hint === 'number' ? 0 : `[stub ${path}]`);
        if (k === Symbol.iterator) return function* () {};
        if (typeof k === 'symbol') return undefined;
        if (Object.prototype.hasOwnProperty.call(t, k)) return Reflect.get(t, k);
        return stub(`${path}.${k}`);
      },
      apply() {
        return stub(`${path}()`);
      },
      construct() {
        return stub(`new ${path}`);
      },
    });
    stubs.set(path, p);
    return p;
  };
  // Re-chequeo de `global_conocido` (main.rs): DIFUZZ_GLOBALS=a,b define esos globals
  // (de la lista de oxc que no existen en node) como stubs, antes de la foto de
  // globals: si la divergencia sigue con ellos definidos, el flag no la explica.
  {
    const names = process.env.DIFUZZ_GLOBALS;
    delete process.env.DIFUZZ_GLOBALS;
    if (names) {
      for (const n of names.split(',')) {
        try {
          if (n && !(n in globalThis)) {
            Object.defineProperty(globalThis, n, { value: stub(n), writable: true, configurable: true, enumerable: false });
          }
        } catch {}
      }
    }
  }

  // Globals que crea el programa (asignaciones implicitas en sloppy): el minifier no
  // puede renombrarlos, y observarlos al salir no agrega usos al fuente.
  const baseGlobals = new Set(Reflect.ownKeys(globalThis));
  const observeNewGlobals = () => {
    for (const k of Reflect.ownKeys(globalThis)) {
      if (baseGlobals.has(k)) continue;
      const d = Object.getOwnPropertyDescriptor(globalThis, k);
      let s;
      try {
        s = d && 'value' in d ? ser(d.value, 0, new Set()) : '[accessor]';
      } catch (e) {
        s = `[throws ${e && e.name}]`;
      }
      ctl('global');
      writeDz(`!dz global ${String(k)} = ${s}\n`);
    }
  };

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
  // t-principio: re-chequeo de `builtins` (recheck::builtins_matter). Cada propiedad pisada se
  // nombra `<objeto> <clave>`: objeto `o<i>`/`p<i>` (WATCHED[i] o su prototype) o `g` (globalThis);
  // clave con encodeURIComponent, o `@@<nombre>` si es un Symbol bien conocido. `-`: no se puede
  // nombrar (Symbol propio).
  const objetoDe = (id) => {
    if (id === 'g') return globalThis;
    const o = WATCHED[Number(id.slice(1))];
    if (!o) return null;
    return id[0] === 'p' ? o.prototype : id[0] === 'o' ? o : null;
  };
  const claveDe = (k) => {
    if (typeof k === 'string') return encodeURIComponent(k);
    for (const n of Reflect.ownKeys(Symbol)) {
      if (typeof n === 'string' && Symbol[n] === k) return `@@${n}`;
    }
    return null;
  };
  const leerClave = (s) => (s.startsWith('@@') ? Symbol[s.slice(2)] : decodeURIComponent(s));
  // DIFUZZ_BUILTINS_FIJOS=<objeto> <clave>,...: antes de la foto, cada una queda como accessor no
  // configurable con el valor original; las escrituras del programa no hacen nada (en estricto
  // tampoco lanzan: hay setter). Redefinirla o borrarla si lanza: el camino cambia y el re-chequeo
  // lo ve. Lo que no se puede fijar sale como `builtin_no_fijado` (inconcluso).
  {
    const fijos = process.env.DIFUZZ_BUILTINS_FIJOS;
    delete process.env.DIFUZZ_BUILTINS_FIJOS;
    if (fijos) {
      for (const spec of fijos.split(',')) {
        try {
          const [id, ks] = spec.split(' ');
          const o = objetoDe(id);
          const k = leerClave(ks);
          if (!o || k === undefined) throw new Error('sin objeto');
          const d = Object.getOwnPropertyDescriptor(o, k);
          if (d && !('value' in d)) {
            Object.defineProperty(o, k, { get: d.get, set: d.set, enumerable: d.enumerable, configurable: false });
          } else {
            const v = d ? d.value : undefined;
            Object.defineProperty(o, k, { get: () => v, set: () => {}, enumerable: d ? d.enumerable : false, configurable: false });
          }
        } catch {
          ctl(`builtin_no_fijado ${spec}`);
        }
      }
    }
  }
  // Lo pisado respecto de la foto: claves cambiadas, borradas o nuevas.
  const pisados = (antes) => {
    const out = [];
    const nombre = (o, k) => {
      let id = o === globalThis ? 'g' : null;
      WATCHED.forEach((w, i) => {
        if (id === null && w === o) id = `o${i}`;
        else if (id === null && w.prototype === o) id = `p${i}`;
      });
      const c = claveDe(k);
      out.push(id && c ? `${id} ${c}` : '-');
    };
    const vistas = new Map();
    for (const [o, k, shot] of antes) {
      if (k === null) continue;
      if (!vistas.has(o)) vistas.set(o, new Set());
      vistas.get(o).add(k);
      const now = describe(o, k);
      const cambio = (now === null) !== (shot === null) ||
        (now && (now[0] !== shot[0] || now[1] !== shot[1] || now[2] !== shot[2]) && !(Number.isNaN(now[0]) && Number.isNaN(shot[0])));
      if (cambio) nombre(o, k);
    }
    for (const [o, ks] of vistas) {
      if (o === globalThis) continue;
      for (const k of Reflect.ownKeys(o)) if (!ks.has(k)) nombre(o, k);
    }
    return out;
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
    observeNewGlobals();
    try {
      if (builtinsChanged()) flags.add('builtins');
    } catch {
      flags.add('builtins');
    }
    // t-principio: que se piso, para el re-chequeo de `builtins` (`builtin <objeto> <clave>`).
    if (flags.has('builtins')) {
      let ps;
      try {
        ps = pisados(before);
      } catch {
        ps = [];
      }
      if (!ps.length) ps = ['-'];
      for (const p of new Set(ps)) ctl(`builtin ${p}`);
    }
    // b4-a: una violacion atrapada (o la de Symbol) es un flag por clase y una linea `viol`.
    const FLAG_VIOL = { tdz: 'tdz_atrapada', symbol: 'coercion_symbol', extends: 'extends_atrapado' };
    for (const [k, pos, , noAtrapada] of violaciones) {
      if (!noAtrapada) flags.add(FLAG_VIOL[k]);
      ctl(`viol ${k} ${pos}`);
    }
    if (sensorTope) ctl('sensor_tope');
    ctl(`flags ${[...flags].sort().join(',')}`);
  });

  // --- Mejora 7: invocar las funciones observadas (`--invocar`) ---------------
  // Con --invocar el epilogo llama a __difuzzObserveInvocar en vez de __difuzzObserve.
  // Primero imprime exactamente lo mismo que __difuzzObserve; despues llama a cada
  // funcion observada, en orden, y escribe `<nombre>() = <resultado>` o
  // `<nombre>() throws <tipo>`. Es solido porque las funciones ya escapan: el epilogo
  // se las pasa a un global desconocido, asi que el minifier no puede asumir como se
  // las llama.
  //   - Sin argumentos y con this undefined: lo unico que no inventa nada. En sloppy
  //     this pasa a ser globalThis, igual en A y en B.
  //   - Sin `new`: una clase lanza TypeError al llamarla (determinista).
  //   - El resultado pasa por `ser`: una funcion devuelta es 'fn', sin nombre ni length.
  //     El tipo de una excepcion sale de thrownType: sin mensaje (trae nombres).
  //   - Promesa nativa: se imprime `promesa` y su resultado sale al terminar
  //     (`!dz promesa <nombre>() ...`), en orden de observacion y no de resolucion: el
  //     minifier puede cambiar cuantos ticks tarda (un await de mas o de menos) y eso
  //     no es observable por contrato. El handler ademas evita que un rechazo mate el
  //     proceso como unhandledRejection.
  //   - Generadores: el objeto se serializa sin iterar (su cuerpo no corre).
  //   - Efectos (timers, I/O, process.exit): no se suprimen; son parte del programa y
  //     corren igual en A y en B. Un loop infinito lo corta el timeout del runner
  //     (el caso cae en observado_sin_salida).
  //   - TDZ al llamar: mismo flag que un throw no atrapado (onThrow).
  const observe = globalThis.__difuzzObserve;
  const promiseThen = Promise.prototype.then;
  const RealPromise = Promise;
  const promesas = [];
  const serSafe = (v) => {
    try {
      return ser(v, 0, new Set());
    } catch (e) {
      return `[throws ${thrownType(e)}]`;
    }
  };
  const invocar = (name, f) => {
    let r;
    const nViol = violaciones.length;
    try {
      r = Reflect.apply(f, undefined, []);
    } catch (e) {
      try {
        if (e instanceof ReferenceError && / before initialization/.test(e.message)) flags.add('tdz');
      } catch {}
      extendsFlag(e);
      // b4-a: la atrapa el preload, no el programa: para el sensor del inspector fue
      // "atrapada", pero cuenta como no atrapada (`tdz`, `extends`). La de Symbol se queda.
      const k = violacionDe(e);
      if (k === 'tdz' || k === 'extends') {
        for (let i = violaciones.length - 1; i >= nViol; i--) {
          if (violaciones[i][0] === k) {
            violaciones.splice(i, 1);
            break;
          }
        }
      }
      // I2: como en onThrow, la TDZ no atrapada se ubica para el re-chequeo.
      if (k === 'tdz') registrarViolacion('tdz', posDeThrow(e), null, true);
      return `${name}() throws ${thrownType(e)}`;
    }
    let isPromise = false;
    try {
      isPromise = r instanceof RealPromise;
    } catch {}
    if (isPromise) {
      const p = { name, s: 'pendiente' };
      promesas.push(p);
      promiseThen.call(
        r,
        (v) => {
          p.s = `= ${serSafe(v)}`;
        },
        (e) => {
          p.s = `throws ${thrownType(e)}`;
        },
      );
      return `${name}() = promesa`;
    }
    return `${name}() = ${serSafe(r)}`;
  };
  // M31 (c): lo que vale cada binding no-funcion antes de invocar, para re-observarlo
  // despues (__difuzzObserveDespues): una funcion invocada puede cambiar el estado.
  const antes = new Map();
  Object.defineProperty(globalThis, '__difuzzObserveInvocar', {
    value: (...pairs) => {
      observe(...pairs);
      for (const [name, value] of pairs) {
        if (typeof value !== 'function') antes.set(String(name), serSafe(value));
      }
      for (const [name, value] of pairs) {
        if (typeof value === 'function') write(`${invocar(name, value)}\n`);
      }
    },
    enumerable: false,
  });
  // M31 (c): el epilogo la llama despues de __difuzzObserveInvocar con los bindings
  // var/let/const, leidos de nuevo (un primitivo reasignado se ve). Imprime solo los
  // no-funcion que cambiaron. Lo que cambie despues (promesas) no se ve.
  Object.defineProperty(globalThis, '__difuzzObserveDespues', {
    value: (...pairs) => {
      for (const [name, value] of pairs) {
        const prev = antes.get(String(name));
        if (typeof value === 'function' || prev === undefined) continue;
        const s = serSafe(value);
        if (s !== prev) write(`${name} = ${s} (tras invocar)\n`);
      }
    },
    enumerable: false,
  });
  baseGlobals.add('__difuzzObserveDespues');
  // M36: `module.exports` del carril A (UMD/CJS sin bindings top-level). Se imprime como
  // un binding mas (`module.exports = ...`, con ser). Con --invocar se llama a lo
  // exportado: la exportacion misma si es funcion, y sus propiedades propias que sean
  // funciones (data descriptors, sin getters), hasta MAX_INVOKED, sin `new`.
  const MAX_INVOKED = 30;
  Object.defineProperty(globalThis, '__difuzzObserveCjs', {
    value: (ex, inv) => {
      observe(['module.exports', ex]);
      if (!inv) return;
      if (typeof ex === 'function') write(`${invocar('module.exports', ex)}\n`);
      if (ex === null || (typeof ex !== 'object' && typeof ex !== 'function')) return;
      const antesCjs = serSafe(ex);
      let n = 0;
      for (const k of Reflect.ownKeys(ex)) {
        if (typeof k !== 'string') continue;
        let d;
        try {
          d = Object.getOwnPropertyDescriptor(ex, k);
        } catch {
          continue;
        }
        if (!d || !('value' in d) || typeof d.value !== 'function') continue;
        if (n++ === MAX_INVOKED) {
          writeDz(`!dz invocadas_truncadas\n`);
          break;
        }
        write(`${invocar(`module.exports.${k}`, d.value)}\n`);
      }
      // M31 (c): el estado exportado despues de invocar, si cambio.
      const despuesCjs = serSafe(ex);
      if (despuesCjs !== antesCjs) write(`module.exports = ${despuesCjs} (tras invocar)\n`);
    },
    enumerable: false,
  });
  baseGlobals.add('__difuzzObserveCjs');

  // --- Carril B: modulos ES (.mjs) -------------------------------------------
  // Observador de modulos: los bindings top-level de un modulo no son globales. El runner
  // corre un envoltorio que importa el programa y pasa su namespace de exports aca. El
  // fuente no cambia (sin efecto observador): los exports ya son usados por ser exports.
  // El namespace trae los nombres ordenados; un binding en TDZ se lee como [throws ...].
  Object.defineProperty(globalThis, '__difuzzObserveExports', {
    value: (ns) => {
      for (const k of Reflect.ownKeys(ns)) {
        if (typeof k !== 'string') continue;
        let s;
        try {
          s = ser(ns[k], 0, new Set());
        } catch (e) {
          s = `[throws ${e && e.name}]`;
        }
        write(`export ${k} = ${s}\n`);
      }
    },
    enumerable: false,
  });
  // No es un global del programa (observeNewGlobals).
  baseGlobals.add('__difuzzObserveInvocar');
  process.on('exit', () => {
    for (const p of promesas) writeDz(`!dz promesa ${p.name}() ${p.s}\n`);
  });
  // M12: con --invocar, lo mismo y despues llama a cada funcion exportada, como
  // __difuzzObserveInvocar con los bindings top-level.
  Object.defineProperty(globalThis, '__difuzzObserveExportsInvocar', {
    value: (ns) => {
      globalThis.__difuzzObserveExports(ns);
      const leer = (k) => {
        try {
          return { v: ns[k] };
        } catch {
          return null;
        }
      };
      const keys = Reflect.ownKeys(ns).filter((k) => typeof k === 'string');
      const antesNs = keys.map((k) => {
        const r = leer(k);
        return r === null || typeof r.v === 'function' ? null : serSafe(r.v);
      });
      for (const k of keys) {
        const r = leer(k);
        if (r !== null && typeof r.v === 'function') write(`${invocar(`export ${k}`, r.v)}\n`);
      }
      // M31 (c): los exports son bindings vivos: los no-funcion que cambiaron al invocar.
      keys.forEach((k, i) => {
        if (antesNs[i] === null) return;
        const r = leer(k);
        const s = r === null ? '[throws]' : serSafe(r.v);
        if (s !== antesNs[i]) write(`export ${k} = ${s} (tras invocar)\n`);
      });
    },
    enumerable: false,
  });
  // Definidos despues de la foto de globals: no son globals del programa.
  baseGlobals.add('__difuzzObserveExports');
  baseGlobals.add('__difuzzObserveExportsInvocar');

  // Un modulo que no enlaza (import que no resuelve, export pedido que no existe) falla
  // antes de ejecutar nada: no es un caso, y no es un throw comparable. Sale con 71
  // (runner::LINK_EXIT). M9: tambien en el carril A, con el `require`/`import()` de algo
  // que no existe (MODULE_NOT_FOUND) no atrapado: antes contaba como "igual" con
  // `lanzaron` sin haber ejecutado casi nada.
  {
    const LINK_CODES = [
      'ERR_MODULE_NOT_FOUND', 'ERR_UNSUPPORTED_DIR_IMPORT', 'ERR_UNSUPPORTED_ESM_URL_SCHEME', 'MODULE_NOT_FOUND',
    ];
    const onLinkError = (e) => {
      let link = null;
      try {
        if (e !== null && typeof e === 'object' && LINK_CODES.includes(e.code)) link = e.code;
        else if (e instanceof SyntaxError && / does not provide an export named /.test(e.message)) link = 'export';
      } catch {}
      if (link === null) return;
      ctl(`link ${link}`);
      writeDz(`!dz link ${link}\n`);
      realExit(71);
    };
    process.prependListener('uncaughtException', onLinkError);
    process.prependListener('unhandledRejection', onLinkError);

    // M30 (`--stubs`, opt-in): un specifier que no resuelve pasa a un modulo stub en
    // vez de cortar el enlace. DIFUZZ_STUBS es la ruta del manifiesto (src/stubs.rs):
    // `{specifier: [nombres]}`, calculado del original y el mismo para A y B. Sin la
    // variable no se registra nada (comportamiento de siempre).
    const stubsPath = process.env.DIFUZZ_STUBS;
    delete process.env.DIFUZZ_STUBS;
    if (stubsPath) {
      let manifest = {};
      try {
        manifest = JSON.parse(fs.readFileSync(stubsPath, 'utf8'));
      } catch {}
      const STUB = Symbol.for('difuzz.stub');
      Object.defineProperty(globalThis, STUB, { value: stub, enumerable: false });
      baseGlobals.add(STUB);
      const PREFIX = 'difuzz-stub:';
      try {
        require('module').registerHooks({
          resolve(spec, context, next) {
            sensor(false);
            try {
              return next(spec, context);
            } catch (e) {
              let code = null;
              try {
                code = e && e.code;
              } catch {}
              if (!LINK_CODES.includes(code)) throw e;
              const cjs = (context.conditions || []).includes('require');
              return { url: `${PREFIX}${cjs ? 'cjs' : 'esm'}/${encodeURIComponent(spec)}`, shortCircuit: true };
            } finally {
              sensor(true);
            }
          },
          load(url, context, next) {
            if (!url.startsWith(PREFIX)) return next(url, context);
            const rest = url.slice(PREFIX.length);
            const spec = decodeURIComponent(rest.slice(4));
            const make = `globalThis[Symbol.for('difuzz.stub')](${JSON.stringify(spec)})`;
            if (rest.startsWith('cjs/')) {
              return { format: 'commonjs', source: `module.exports = ${make};\n`, shortCircuit: true };
            }
            const names = Array.isArray(manifest[spec]) ? manifest[spec] : [];
            let src = `const s = ${make};\nexport default s;\n`;
            names.forEach((n, i) => {
              if (typeof n !== 'string' || n === 'default') return;
              src += `const n${i} = s[${JSON.stringify(n)}];\nexport { n${i} as ${JSON.stringify(n)} };\n`;
            });
            return { format: 'module', source: src, shortCircuit: true };
          },
        });
      } catch {}
    }
  }

  // --- b4-a (R1-H04 / C3): violaciones atrapadas --------------------------------
  // onThrow solo ve lo NO atrapado: `try { x; } catch {} let x;` no prendia `tdz`, y lo
  // mismo con `Math.abs(Symbol())` o `class A extends v {}` atrapados. El inspector de V8 en
  // este mismo proceso pausa en cada excepcion ATRAPADA (state 'caught'), sincronicamente:
  // el handler mira la descripcion, registra la violacion y sigue. Va al final del preload
  // para no ver los throws del preload mismo, y descarta los que lanza codigo del preload
  // (p. ej. leer un export en TDZ al observar) o de node (url que no es file: del programa).
  // Solo observa: no escribe en stdout ni toca objetos del programa. Efectos conocidos:
  //   - al pausar, V8 arma la descripcion del Error leyendo `.stack` (MEDIDO): el stack
  //     queda formateado en el throw (si despues el programa cambia `message`, ve el
  //     viejo), llama a Error.prepareStackTrace (o a la del programa, si la cambio) y
  //     prende `stack` (ver prepareStackTrace).
  //   - Debugger.enable cuesta ~12 ms por node y cada pausa ~0.4 ms: tope MAX_PAUSAS.
  // Igual en A y en B: lo que cambie, cambia en los dos.
  // DIFUZZ_SENSOR=0: sin sensor (runner.rs lo pide si node murio por una senal con el).
  const conSensor = process.env.DIFUZZ_SENSOR !== '0';
  delete process.env.DIFUZZ_SENSOR;
  if (conSensor) try {
    const PRELOAD_URL = require('url').pathToFileURL(__filename).href;
    const inspector = require('inspector');
    const sesion = new inspector.Session();
    sesion.connect();
    const MAX_PAUSAS = 256;
    let pausas = 0;
    const DESC = [
      ['tdz', /^ReferenceError: [^\n]* before initialization/],
      ['symbol', /^TypeError: Cannot convert a Symbol value to a/],
      ['extends', /^TypeError: Class extends value/],
    ];
    // `callFrames[].url` llega vacio (MEDIDO, node 24): la url sale de scriptParsed. Solo
    // los file: (el programa, el envoltorio del carril B y este preload); eval no.
    const urls = new Map();
    sesion.on('Debugger.scriptParsed', (m) => {
      try {
        const p = m.params;
        if (typeof p.url === 'string' && p.url.startsWith('file:')) urls.set(p.scriptId, p.url);
      } catch {}
    });
    sesion.on('Debugger.paused', (m) => {
      try {
        const p = m.params || {};
        const d = p.data || {};
        const desc = typeof d.description === 'string' ? d.description : '';
        const f = (p.callFrames || [])[0];
        const url = (f && urls.get(f.location.scriptId)) || '';
        if ((p.reason === 'exception' || p.reason === 'promiseRejection') && url && url !== PRELOAD_URL) {
          for (const [k, re] of DESC) {
            if (re.test(desc)) {
              registrarViolacion(k, `${f.location.lineNumber}:${f.location.columnNumber}`, desc);
              break;
            }
          }
        }
      } catch {
      } finally {
        sesion.post('Debugger.resume');
        if (++pausas === MAX_PAUSAS) {
          sensorTope = true;
          sesion.post('Debugger.setPauseOnExceptions', { state: 'none' });
        }
      }
    });
    sensor = (on) => sesion.post('Debugger.setPauseOnExceptions', { state: on && pausas < MAX_PAUSAS ? 'caught' : 'none' });
    sesion.post('Debugger.enable');
    // Con el debugger prendido, cada `debugger;` del programa pausa (MEDIDO: un loop con
    // `debugger;` y --invocar paso a Timeout). Sin breakpoints activos no pausa, y las
    // pausas por excepcion siguen (MEDIDO).
    sesion.post('Debugger.setBreakpointsActive', { active: false });
    sensor(true);
  } catch {}
})();
