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
  delete process.env.DIFUZZ_SALT;

  // Node usa primordials internos: console.log(f) no pasa por aca, solo el programa.
  const realToString = Function.prototype.toString;
  Object.defineProperty(Function.prototype, 'toString', {
    value: {
      toString() {
        flags.add('fn_tostring');
        const s = realToString.call(this);
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
  Error.prepareStackTrace = (err, frames) => {
    if (!quiet) flags.add('stack');
    let head;
    try {
      head = String(err);
    } catch {
      head = 'Error';
    }
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
    try {
      r = Reflect.apply(f, undefined, []);
    } catch (e) {
      try {
        if (e instanceof ReferenceError && / before initialization/.test(e.message)) flags.add('tdz');
      } catch {}
      extendsFlag(e);
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
})();
