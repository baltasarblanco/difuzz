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

  // Se captura antes de que el programa pueda pisar console.log o process.stdout.write.
  const write = process.stdout.write.bind(process.stdout);
  const writeErr = process.stderr.write.bind(process.stderr);

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

  // V8 llama a prepareStackTrace la primera vez que alguien lee .stack.
  Error.prepareStackTrace = (err, frames) => {
    flags.add('stack');
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

  // stderr no se compara: lo que el programa escribe por console.error/warn/trace pasaria
  // desapercibido (y un programa que solo usa esos contaria como mudo). trace sin stack.
  const util = require('util');
  for (const m of ['error', 'warn', 'trace']) {
    console[m] = (...args) => {
      write(`[${m}] ${util.format(...args)}\n`);
    };
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
      write(`!dz global ${String(k)} = ${s}\n`);
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
    writeErr(`\n__DIFUZZ_FLAGS__ ${[...flags].sort().join(',')}\n`);
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
  Object.defineProperty(globalThis, '__difuzzObserveInvocar', {
    value: (...pairs) => {
      observe(...pairs);
      for (const [name, value] of pairs) {
        if (typeof value === 'function') write(`${invocar(name, value)}\n`);
      }
    },
    enumerable: false,
  });
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
    for (const p of promesas) write(`!dz promesa ${p.name}() ${p.s}\n`);
  });
  // Definido despues de la foto de globals: no es un global del programa.
  baseGlobals.add('__difuzzObserveExports');

  // Un modulo que no enlaza (import que no resuelve, export pedido que no existe) falla
  // antes de ejecutar nada: no es un caso, y no es un throw comparable. Sale con 71
  // (runner::LINK_EXIT). Solo en .mjs: el carril A queda como estaba.
  if (/\.mjs$/.test(process.argv[1] || '')) {
    const LINK_CODES = ['ERR_MODULE_NOT_FOUND', 'ERR_UNSUPPORTED_DIR_IMPORT', 'ERR_UNSUPPORTED_ESM_URL_SCHEME'];
    const onLinkError = (e) => {
      let link = null;
      try {
        if (e !== null && typeof e === 'object' && LINK_CODES.includes(e.code)) link = e.code;
        else if (e instanceof SyntaxError && / does not provide an export named /.test(e.message)) link = 'export';
      } catch {}
      if (link === null) return;
      write(`!dz link ${link}\n`);
      process.exit(71);
    };
    process.prependListener('uncaughtException', onLinkError);
    process.prependListener('unhandledRejection', onLinkError);
  }
})();
