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
  // 7.2: pid y ppid inventados e IMPOSIBLES: mas que PID_MAX_LIMIT de Linux de 64 bits
  // (4194304, include/linux/threads.h; pid_max no puede pasarlo), asi que ningun proceso
  // los tiene y lo que se los pase al sistema (`process._debugProcess`, `os.getPriority`,
  // `/proc/<pid>`) da ESRCH/ENOENT en vez de llegar a un proceso ajeno. Antes eran 4242 y
  // 4241: sin --sandbox, `process.kill(process.pid, 'SIGTERM')` le pegaba al proceso 4242
  // de la maquina, y con --sandbox el filtro de senales lo negaba (EPERM) y marcaba el caso.
  // Enteros de 32 bits: `process.kill` rechaza un pid que no lo es.
  const PID_FICTICIO = 4204242;
  const PPID_FICTICIO = 4204241;
  const pidReal = process.pid;
  fix(process, 'pid', PID_FICTICIO);
  fix(process, 'ppid', PPID_FICTICIO);
  // 7.2: `process.kill` llama a `process._kill(pid, senal)` (node 24, la busca en cada
  // llamada: MEDIDO con process.kill.toString()), que devuelve 0 o -errno. Se envuelve
  // `_kill` y no `kill`: asi `process.kill` queda intacto (su toString, length, name y
  // sus errores, que arma node con lo que devuelve `_kill`) y el camino directo por
  // `process._kill` tambien pasa por aca. El pid ficticio (y su grupo, el negativo) pasa
  // a ser el real: matarse, la senal 0 a si mismo y un handler propio andan como en node,
  // y con --sandbox el filtro lo deja (no marca). El ppid ficticio (y su negativo) da
  // ESRCH sin llamar al sistema: NUNCA el padre real (difuzz); es lo que da un pid que no
  // existe aunque la senal sea invalida (MEDIDO: `_kill(<pid inexistente>, 999)` = -3).
  // El resto, incluido 0 (el grupo propio), va tal cual. El pid se convierte como lo hace
  // node (ToNumber y despues a int32: `_kill(pid + 2 ** 32, 0)` es `_kill(pid, 0)`,
  // MEDIDO), una sola vez, y la senal la convierte node despues, en el mismo orden.
  // Lo que el programa puede ver de `_kill` queda igual: nombre, length 0, sin
  // `prototype`, no es constructor, el mismo descriptor y el toString nativo (ver el
  // toString de abajo). Diferencia aceptada: un error al convertir el pid (un Symbol) trae
  // un marco del preload en el stack.
  let killReal = null;
  let killPropio = null;
  try {
    const d = Object.getOwnPropertyDescriptor(process, '_kill');
    if (d && 'value' in d && typeof d.value === 'function') {
      const real = d.value;
      const aplicar = Reflect.apply;
      let esrch = -3;
      try {
        esrch = -require('os').constants.errno.ESRCH;
      } catch {}
      // 7.7 (MENOR 4): -EPERM del `_kill` nativo (UV_EPERM, -1 en Linux).
      let eperm = -1;
      try {
        eperm = -require('os').constants.errno.EPERM;
      } catch {}
      const propio = {
        _kill(pid, sig) {
          let p = +pid | 0;
          if (p !== pidReal && p !== -pidReal) {
            if (p === PID_FICTICIO) p = pidReal;
            else if (p === -PID_FICTICIO) p = -pidReal;
            else if (p === PPID_FICTICIO || p === -PPID_FICTICIO) {
              void +sig;
              return esrch;
            }
          }
          const r = aplicar(real, this, [p, sig]);
          // 7.7 (MENOR 4): con --permission, un EPERM del `_kill` nativo casi siempre es el
          // filtro seccomp que nego la senal (runner::encerrar_senales): marca el caso aunque
          // el programa no lance (`_kill` directo) o atrape el `kill EPERM` sin el sensor. No
          // se distingue de un EPERM natural (un proceso de otro usuario, p. ej. el pid 1):
          // ese tambien marca (rev-int H5, MEDIDO; README, sandbox): con 7.9, si el original y el
          // minificado lo hacen, el caso se compara con la nota (antes iba a descartes).
          // `conPermisos` y `ctl` se definen mas abajo; aca solo se leen en la llamada.
          if (r === eperm && conPermisos) ctl('sandbox');
          return r;
        },
      }._kill;
      Object.defineProperty(propio, 'length', { value: real.length });
      Object.defineProperty(process, '_kill', { ...d, value: propio });
      killReal = real;
      killPropio = propio;
    }
  } catch {}
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
  // b7-1c (revision 7.1b, S2): lo que usan el canal y el calculo de la cobertura (al salir, despues
  // del programa) se toma ahora: el programa puede pisar `fs.writeSync`, los metodos de String o
  // de Number, `Math.imul`, `Object.keys`, el iterador de los arreglos (`for...of`) o
  // `Session.prototype.post`, y con eso callar una linea del canal o cambiar lo que entra al hash
  // (rev-7.1b x7, x8, x8b, x8c). `desligar(f)(x, ...a)` llama `f` con `this` = x sin buscar
  // `call` en el momento.
  const desligar = (f) => Function.prototype.call.bind(f);
  const writeSync = desligar(fs.writeSync);
  const strCharCodeAt = desligar(String.prototype.charCodeAt);
  const strIndexOf = desligar(String.prototype.indexOf);
  const strSlice = desligar(String.prototype.slice);
  const strEmpiezaEn = desligar(String.prototype.startsWith);
  const imul = Math.imul;
  const esArreglo = Array.isArray;
  const clavesPropias = Object.keys;
  const crearObjeto = Object.create;
  let ctlFd = null;
  try {
    if (process.env.DIFUZZ_CTL) ctlFd = fs.openSync(process.env.DIFUZZ_CTL, 'a');
  } catch {}
  delete process.env.DIFUZZ_CTL;
  const ctl = (line) => {
    if (ctlFd === null) return;
    try {
      writeSync(fs, ctlFd, `${line}\n`);
    } catch {}
  };
  // Una linea "!dz " del preload: se cuenta en el canal (runner::Exec::own_output).
  const writeDz = (s) => {
    ctl('dz');
    write(s);
  };
  // n-sandbox (`--sandbox`): un permiso que el modelo de permisos de node nego (ERR_ACCESS_DENIED;
  // ERR_DLOPEN_DISABLED, el addon nativo; kill EPERM, una senal; uv_os_setpriority EPERM, la prioridad de otro
  // proceso) va al canal como `sandbox`: si le paso al original,
  // runner::Exec::toco_sandbox lo ve y main::toca_sandbox decide: si lo toco solo el original
  // (2c) o el original y el minificado (7.9) se compara, con una nota en meta.txt; si el
  // minificado no llego a correr, el caso va a descartes. Solo con --permission
  // (`process.permission` no existe sin el, MEDIDO node 24): sin el flag no se lee nada mas.
  // Sin getters ni trampas del programa: el descriptor propio (node pone `code` como dato, MEDIDO)
  // y nunca sobre un Proxy. Las funciones se toman ahora, antes de que corra el programa.
  const conPermisos = typeof process.permission === 'object' && process.permission !== null;
  const descriptorPropio = Object.getOwnPropertyDescriptor;
  const esProxy = require('util').types.isProxy;
  // 8.2: devuelve si anoto (onThrow lo usa para `sandbox_sin_atrapar`).
  const notarSandbox = (e) => {
    if (!conPermisos) return false;
    let anoto = false;
    const marcar = () => {
      ctl('sandbox');
      anoto = true;
    };
    try {
      if (e === null || (typeof e !== 'object' && typeof e !== 'function') || esProxy(e)) return false;
      const d = descriptorPropio(e, 'code');
      if (d && (d.value === 'ERR_ACCESS_DENIED' || d.value === 'ERR_DLOPEN_DISABLED')) marcar();
      // 2b: una senal a otro proceso que el filtro seccomp nego (runner::encerrar_senales):
      // `code` EPERM y `syscall` kill, los dos datos propios (MEDIDO node 24.14.1; tambien
      // `process._debugProcess`, que lanza `EPERM, Operation not permitted` con `syscall` kill).
      const s = descriptorPropio(e, 'syscall');
      if (d && d.value === 'EPERM' && s && s.value === 'kill') marcar();
      // 7.7: `os.setPriority` a otro proceso, negado por el mismo filtro: SystemError con
      // `code` ERR_SYSTEM_ERROR e `info` { code: 'EPERM', syscall: 'uv_os_setpriority' },
      // todos datos propios (MEDIDO node 24.14.1; `errno` y `syscall` del error son getters).
      const info = descriptorPropio(e, 'info');
      const i = info && info.value;
      if (d && d.value === 'ERR_SYSTEM_ERROR' && i !== null && typeof i === 'object' && !esProxy(i)) {
        const ic = descriptorPropio(i, 'code');
        const is = descriptorPropio(i, 'syscall');
        if (ic && ic.value === 'EPERM' && is && is.value === 'uv_os_setpriority') marcar();
      }
    } catch {}
    return anoto;
  };
  // 7.7 (revision fe0cd86, MENOR 4): las senales negadas que el programa atrapa sin el
  // sensor. `process._kill` (que no lanza) marca en su envoltorio (7.2, arriba).
  // `process._debugProcess` (SIGUSR1 a otro pid) lanza `EPERM, Operation not permitted`
  // con `syscall` kill. Envuelto solo con --permission (sin el, ni se toca), con los mismos
  // atributos y `length` (0) que el de node.
  if (conPermisos) {
    const aplicar = Reflect.apply;
    const envolver = (key, hacer) => {
      try {
        const desc = descriptorPropio(process, key);
        if (!desc || typeof desc.value !== 'function') return;
        Object.defineProperty(process, key, { ...desc, value: hacer(desc.value) });
      } catch {}
    };
    envolver('_debugProcess', (real) => ({
      _debugProcess(...args) {
        try {
          return aplicar(real, this, args);
        } catch (e) {
          notarSandbox(e);
          throw e;
        }
      },
    })._debugProcess);
  }

  // --- Sensor de asunciones ---------------------------------------------------
  const flags = new Set();
  // Corrida "salada" (runner::run_salted): mismo programa, otro contenido de
  // toString y .stack. Se borra del entorno para que el programa no la vea.
  const SALT = process.env.DIFUZZ_SALT === '1';
  // t-principio: contenido FIJO (re-chequeo de contenido, recheck::contenido_matters): el mismo
  // toString y el mismo .stack para cualquier codigo, asi A' y B' no dependen del texto.
  const FIJO = process.env.DIFUZZ_SALT === 'fijo';
  delete process.env.DIFUZZ_SALT;
  // b7-1b: corrida con cobertura (recheck::camino_*): la cobertura de bloques (al final), los
  // ganchos del minificado instrumentado y el sensor de uso (antes de la foto de globals).
  const COBERTURA = process.env.DIFUZZ_COBERTURA === '1';
  delete process.env.DIFUZZ_COBERTURA;
  // b7-1c (revision 7.1b, S2 y P1): el texto que corre esta instrumentado (recheck::GANCHOS): solo
  // entonces se definen los ganchos (abajo) y `toString` saca lo insertado (`quitarGanchos`).
  const GANCHOS = COBERTURA && process.env.DIFUZZ_GANCHOS === '1';
  delete process.env.DIFUZZ_GANCHOS;
  // Las cuentas de `__difuzzNeutroRama` (abajo), para el hash de la cobertura.
  let cuentasDeRamas = null;
  // b7-1b: lo que usan los ganchos, tomado antes de que corra el programa (corren en medio
  // del programa, que puede pisar los built-ins).
  const aplicar = Reflect.apply;
  const construir = Reflect.construct;
  const tiene = Reflect.has;
  const ObjetoReal = Object;
  const definir = Object.defineProperty;
  const strEndsWith = String.prototype.endsWith;
  const strStartsWith = String.prototype.startsWith;
  const termina = (s, x) => aplicar(strEndsWith, s, [x]);
  const empieza = (s, x) => aplicar(strStartsWith, s, [x]);
  // b7-1b (revision 7.1, H2): sensor de uso. `uso <que>`: B toco la asuncion (leyo el global
  // conocido, resolvio un nombre en el objeto de un `with`, llamo a un helper de coercion, leyo
  // un built-in despues de pisarlo, uso el `this` de sloppy); `uso? <que>`: no se pudo sensar.
  // recheck::comparar_caminos: con cualquiera de las dos, B no es concluyente. Sin repetir y con
  // tope (16): con una alcanza.
  const usos = Object.create(null);
  let nUsos = 0;
  const marcarUso = (s) => {
    if (usos[s] === true || nUsos >= 16) return;
    usos[s] = true;
    nUsos++;
    ctl(s);
  };
  const uso = (s) => marcarUso(`uso ${s}`);
  const usoNoSensado = (s) => marcarUso(`uso? ${s}`);

  // b7-1c (revision 7.1b, P1): lo que recheck::instrumentar_ramas y recheck::interruptores_sloppy
  // insertan en el texto de B (recheck::RAMA_FIN, recheck::THIS_INSERTO): con GANCHOS, el texto
  // de una funcion sale sin eso, es decir, el de B. El fuente no nombra `__difuzzNeutro`
  // (recheck::instrumentar se niega), asi que todo lo que se saca es lo insertado.
  const MARCA_GANCHO = '__difuzzNeutro';
  const RAMA_INICIO = '(__difuzzNeutroRama(';
  const RAMA_FIN = ' /*__difuzzNeutroFin*/)';
  const THIS_INSERTO = ';__difuzzNeutroThis(this);';
  const quitarGanchos = (s) => {
    if (typeof s !== 'string' || strIndexOf(s, MARCA_GANCHO) < 0) return s;
    let out = '';
    let i = 0;
    for (;;) {
      const j = strIndexOf(s, MARCA_GANCHO, i);
      if (j < 0) break;
      if (j - 1 >= i && strEmpiezaEn(s, RAMA_INICIO, j - 1)) {
        const d = j - 1 + RAMA_INICIO.length;
        let k = d;
        while (k < s.length && strCharCodeAt(s, k) >= 48 && strCharCodeAt(s, k) <= 57) k++;
        if (k > d && strEmpiezaEn(s, '),', k)) {
          out += strSlice(s, i, j - 1);
          i = k + 2;
          continue;
        }
      } else if (j - 3 >= i && strEmpiezaEn(s, RAMA_FIN, j - 3)) {
        out += strSlice(s, i, j - 3);
        i = j - 3 + RAMA_FIN.length;
        continue;
      } else if (j - 1 >= i && strEmpiezaEn(s, THIS_INSERTO, j - 1)) {
        out += strSlice(s, i, j - 1);
        i = j - 1 + THIS_INSERTO.length;
        continue;
      }
      out += strSlice(s, i, j + MARCA_GANCHO.length);
      i = j + MARCA_GANCHO.length;
    }
    return out + strSlice(s, i);
  };

  // Node usa primordials internos: console.log(f) no pasa por aca, solo el programa.
  const realToString = Function.prototype.toString;
  const llamarToString = desligar(realToString);
  Object.defineProperty(Function.prototype, 'toString', {
    value: {
      toString() {
        flags.add('fn_tostring');
        // 7.2: `process._kill` envuelto se ve como el nativo. 7.1c: sin buscar `call` en el
        // momento (`llamarToString`), y sin los ganchos insertados en B (`quitarGanchos`).
        const f = killPropio !== null && this === killPropio ? killReal : this;
        const s = GANCHOS ? quitarGanchos(llamarToString(f)) : llamarToString(f);
        // b7-1b (revision 7.1, H2 d05): el texto de una funcion nativa (`function max() {
        // [native code] }`) es el mismo en A y en B: el minifier no lo cambia y la asuncion no
        // habla de el. Fijarlo hacia que un programa que elige un valor por ese texto diera en
        // A' otro valor, que podia coincidir con el del bug en B. Queda el de verdad.
        if (FIJO) return termina(s, '{ [native code] }') ? s : 'function () { [native code] }';
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
  // 7.5 (revision n2, MENOR 5): los archivos de los marcos que se formatean en esta corrida
  // van al canal al salir (`marco <archivo>`, una vez cada uno, hasta MAX_ARCHIVOS_MARCO).
  // oracle::compare borra la posicion de un marco solo si su archivo es uno de estos (o de
  // difuzz): una linea que el programa imprime con forma de marco y otro archivo la conserva.
  // Mismo recorrido que oracle::sin_posiciones, sin metodos que el programa pueda pisar.
  // prepareStackTrace (que corre tambien en las pausas del sensor) solo guarda los marcos
  // formateados (los primeros MAX_STACKS_MARCO stacks); se recorren al salir. Recorrerlos
  // (o escribir el canal) adentro cambiaba un SIGSEGV de node con el sensor
  // (fixture/issues/9460/side-effects, MEDIDO en 12 corridas del original y 12 del minificado:
  // SIGSEGV 12 y 12 con el preload de fe0cd86 y con solo guardar; 5 y 5 escribiendo el canal
  // adentro; 6 y 4 recorriendo adentro sin escribir).
  const MAX_ARCHIVOS_MARCO = 256;
  const MAX_STACKS_MARCO = 1024;
  const stacksDeMarco = [];
  let nStacksDeMarco = 0;
  const archivosDeMarco = Object.create(null);
  let nArchivosDeMarco = 0;
  const esDigito = (c) => c >= '0' && c <= '9';
  const anotarArchivosDeMarco = (f) => {
    let desde = 0;
    for (let i = 0; i < f.length; i++) {
      if (f[i] !== ':') continue;
      let k = i + 1;
      while (k < f.length && esDigito(f[k])) k++;
      if (k === i + 1 || f[k] !== ':') continue;
      let j = k + 1;
      while (j < f.length && esDigito(f[j])) j++;
      if (j === k + 1 || !(j === f.length || f[j] === ')' || f[j] === ',')) continue;
      let ini = i;
      while (ini > desde && f[ini - 1] !== ' ' && f[ini - 1] !== '(') ini--;
      let a = '';
      for (let p = ini; p < i; p++) a += f[p];
      if (a !== '' && archivosDeMarco[a] !== true && nArchivosDeMarco < MAX_ARCHIVOS_MARCO) {
        archivosDeMarco[a] = true;
        nArchivosDeMarco++;
      }
      desde = j;
      i = j - 1;
    }
  };
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
    const lineas = frames.map((f) => `${f}`);
    if (nStacksDeMarco < MAX_STACKS_MARCO) stacksDeMarco[nStacksDeMarco++] = lineas;
    return head + lineas.map((l) => `\n    at ${l}`).join('') + (SALT ? '\n    at dz' : '');
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

  // n-ruido (t-real): los handles de timers de node (`Timeout` de setTimeout/setInterval,
  // `Immediate` de setImmediate) guardan estado del reloj real del event loop: serializados
  // campo por campo, A y B dan numeros distintos sin que el programa cambie. Los prototipos
  // se toman aca de un timer propio (creado y cancelado antes de que corra el programa): un
  // objeto del programa no puede compartirlos, y una clase propia llamada Timeout (cuyo
  // nombre el minifier puede cambiar) no los usa.
  // b7-3 (revision n2, MENOR 4): se ocultan solo los campos que no son del programa
  // (MEDIDO con node 24.14.1, b7-3/medir): `_idleStart` (reloj del loop al armar o
  // refrescar), `_idlePrev`/`_idleNext` (la lista enlazada de node: la TimersList trae
  // `expiry` del reloj, y su `id`, su `priorityQueuePosition` y los vecinos dependen de
  // cuando disparo cada timer) y los ids de async_hooks (`asyncId`, `triggerId`: cuentan
  // todos los recursos asincronos del proceso, y una escritura a un pipe lleno crea uno).
  // El resto se imprime como cualquier objeto: `_idleTimeout` (el delay; -1 cancelado),
  // `_repeat` (setInterval o setTimeout), `_destroyed`, `_onTimeout`/`_onImmediate` (fn o
  // null), los argumentos, `refed` (unref/ref), `kHasPrimitive` y lo que el programa le
  // agrego. Las claves ocultas son las del timer de referencia (los symbols de node son
  // unicos: un symbol del programa con la misma descripcion se imprime).
  const OCULTAS = new Set(['_idleStart', '_idlePrev', '_idleNext', 'asyncId', 'triggerId']);
  const OPACOS = new Map();
  const opacar = (h, nombre) => {
    const ocultas = Reflect.ownKeys(h).filter((k) => OCULTAS.has(typeof k === 'symbol' ? k.description : k));
    OPACOS.set(Object.getPrototypeOf(h), [nombre, new Set(ocultas)]);
  };
  try {
    const t = setTimeout(() => {}, 0);
    opacar(t, '[Timeout]');
    clearTimeout(t);
  } catch {}
  try {
    const i = setImmediate(() => {});
    opacar(i, '[Immediate]');
    clearImmediate(i);
  } catch {}

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
      const opaco = OPACOS.get(Object.getPrototypeOf(v));
      if (opaco) {
        // Mismo formato que un objeto (abajo), sin las claves ocultas.
        const [nombre, ocultas] = opaco;
        const keys = Reflect.ownKeys(v).filter((k) => !ocultas.has(k));
        const out = keys.slice(0, MAX_ITEMS).map((k) => {
          const d = Object.getOwnPropertyDescriptor(v, k);
          const clave = typeof k === 'symbol' ? `[${String(k)}]` : JSON.stringify(k);
          return `${d && !d.enumerable ? '~' : ''}${clave}: ${d && 'value' in d ? ser(d.value, depth + 1, ancestors) : '[accessor]'}`;
        });
        if (keys.length > MAX_ITEMS) out.push(`...${keys.length}`);
        return `${nombre}{${out.join(', ')}}`;
      }
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
  // 8.4: corre `f` con el sensor apagado, con apagados anidados (el hook de --stubs llama al
  // resolve de node, que llama a `Module._findPath` envuelto mas abajo): solo el de afuera lo
  // vuelve a prender. Sin la cuenta, el de adentro lo prenderia antes de tiempo y el sensor
  // pausaria en el MODULE_NOT_FOUND que el hook de --stubs atrapa.
  let sensorApagado = 0;
  const sinSensor = (f) => {
    if (sensorApagado++ === 0) sensor(false);
    try {
      return f();
    } finally {
      if (--sensorApagado === 0) sensor(true);
    }
  };
  // t-sensor: con DIFUZZ_SENSOR=perezoso (runner::puede_atrapar dijo que el programa no
  // puede atrapar una excepcion) el sensor no se prende al arrancar: queda aca y lo prende
  // `invocar` antes de la primera funcion que llama (el preload si atrapa lo que lanza).
  let sensorPerezoso = null;
  // Paso 7 (revision-tint I2): prende el sensor perezoso, si esta pendiente. Ademas de
  // `invocar`, lo llaman `ser` (si hay built-ins pisados: es la unica forma de que la
  // serializacion corra codigo del programa sin nada de la lista de puede_atrapar, como
  // `Map.prototype[@@iterator]` alcanzado con getPrototypeOf) y `onThrow` (con un objeto:
  // `String(e)` puede llamar a un @@toPrimitive propio). Lo que lancen adentro lo atrapa
  // el preload y el sensor tiene que verlo, como con el sensor prendido desde el arranque.
  const prenderPerezoso = () => {
    if (sensorPerezoso === null) return;
    const prender = sensorPerezoso;
    sensorPerezoso = null;
    prender();
  };
  // Paso 7 (I2): antes de serializar (entradas de la observacion: __difuzzObserve, los
  // exports, los globals nuevos, serSafe). Con un built-in pisado, `ser` puede correr codigo
  // del programa (iterar un Map, `Math.min`, `push`/`join`). Una vez por entrada y no en cada
  // `ser`: entre dos bindings de la misma entrada no corre codigo del programa salvo por un
  // pisado (y entonces ya se prendio). builtinsChanged: ~0,1 ms (MEDIDO, node 24, 509 claves).
  // sensorPerezoso solo deja de ser null al final del preload, despues de `before`.
  const prenderSiPisado = () => {
    if (sensorPerezoso === null) return;
    let pisado = true;
    try {
      pisado = builtinsChanged();
    } catch {}
    if (pisado) prenderPerezoso();
  };
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
  // Paso 7 (I2): un Error nativo (prototipo de uno de ERRORS) sin mas claves propias que
  // `stack` y `message`: String(e) y su stack no llegan a codigo del programa salvo por un
  // built-in pisado (Error.prototype esta en la foto) o por lo que esta en la lista de
  // runner::puede_atrapar (prototype, getPrototypeOf, prepareStackTrace, Proxy...).
  const errorNativo = (e) => {
    try {
      const p = Object.getPrototypeOf(e);
      if (!ERRORS.some((E) => E.prototype === p)) return false;
      return Reflect.ownKeys(e).every((k) => k === 'stack' || k === 'message');
    } catch {
      return false;
    }
  };
  const onThrow = (e) => {
    // Paso 7 (I2, revision-tint s04): con un objeto, lo que sigue puede correr codigo del
    // programa (@@toPrimitive, toString propio) adentro de un try del preload.
    if (sensorPerezoso !== null && e !== null && (typeof e === 'object' || typeof e === 'function')) {
      let pisado = true;
      try {
        pisado = builtinsChanged();
      } catch {}
      if (pisado || !errorNativo(e)) prenderPerezoso();
    }
    try {
      if (e instanceof ReferenceError && / before initialization/.test(e.message)) flags.add('tdz');
    } catch {}
    extendsFlag(e);
    // 8.2: lo que mata al programa es un permiso negado: lo que seguia no corre
    // (runner::Runner::murio_por_sandbox; si A y B dan lo mismo, main lo descarta).
    if (notarSandbox(e)) ctl('sandbox_sin_atrapar');
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
      prenderSiPisado();
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
            if (COBERTURA) {
              // b7-1b (revision 7.1, H2 d01): con cobertura (el camino de B con los stubs), un
              // accessor que anota cada lectura: `typeof window` o `window` en B es uso de la
              // asuncion aunque B no cambie de camino. Las escrituras se leen de vuelta.
              let v = stub(n);
              Object.defineProperty(globalThis, n, {
                get() {
                  uso(`global ${n}`);
                  return v;
                },
                set(x) {
                  v = x;
                },
                configurable: true,
                enumerable: false,
              });
            } else {
              Object.defineProperty(globalThis, n, { value: stub(n), writable: true, configurable: true, enumerable: false });
            }
          }
        } catch {}
      }
    }
  }
  // b7-1 (revision n2, IMPORTANTE 1): los interruptores del camino de B (recheck::camino_*).
  // DIFUZZ_NEUTRO=0|1 define dos funciones que el minificado instrumentado llama
  // (recheck::interruptores_with/_coercion): con 0 son la identidad (B tal cual) y con 1
  // neutralizan como el re-chequeo neutraliza el original: el objeto de un `with` pasa a
  // uno sin propiedades (cada nombre resuelve afuera, como `{ o; S; }`) y la clave de un
  // helper de coercion pasa a `__dz_<nombre>` (como neutralize_coercion). Antes de la foto
  // de globals: no son globals del programa. Sin la variable no se define nada.
  // b7-1b (revision 7.1, H1/H2): con DIFUZZ_COBERTURA=1, ademas, los ganchos que pone en el
  // minificado recheck::instrumentar_* y el sensor de uso (b7-1c, S2: los ganchos solo con
  // DIFUZZ_GANCHOS=1, es decir, cuando el texto que corre los usa; antes, en toda corrida con
  // cobertura, y un B sin instrumentar podia verlos):
  //   - `__difuzzNeutroRama(i)`: cuenta cada vez que corre una parte condicional que la
  //     cobertura de V8 no separa (el lado derecho de `&&=`/`||=`/`??=`, el valor por defecto
  //     de un parametro o de una desestructuracion: MEDIDO, node 24.14.1); las cuentas entran
  //     al hash de la cobertura (abajo).
  //   - con DIFUZZ_NEUTRO=0, el objeto de cada `with` pasa a un Proxy que anota cada nombre que
  //     resuelve en el (`uso with <nombre>`); con 1, como antes. Un Proxy cambia el `this` de una
  //     funcion llamada por un nombre del `with`, pero eso ya es uso.
  //   - `__difuzzNeutroUso(k)` (al principio del cuerpo de cada helper de coercion definido como
  //     funcion) y `__difuzzNeutroValor(v, k)` (envuelve el valor de los demas: si es una
  //     funcion, tambien nativa, la llamada se anota): `uso coercion <k>`, salvo con
  //     DIFUZZ_NEUTRO=1 (ahi la clave ya es otra y la coercion no los llama).
  //   - `__difuzzNeutroThis(this)` (al principio de cada funcion sloppy que usa `this`): en la
  //     corrida en estricto, un `this` que sloppy hubiera cambiado (undefined, null, primitivo)
  //     es `uso sloppy this`. En sloppy nunca lo es.
  {
    const neutro = process.env.DIFUZZ_NEUTRO;
    delete process.env.DIFUZZ_NEUTRO;
    const def = (k, v) => Object.defineProperty(globalThis, k, { value: v, writable: false, configurable: false, enumerable: false });
    const PROPIO = '__difuzzNeutro';
    if (neutro === '0' || neutro === '1') {
      const n = neutro === '1';
      try {
        def('__difuzzNeutroWith', (o) => {
          if (n) return crearObjeto(null);
          if (!COBERTURA || o === null || o === undefined) return o;
          return new RealProxy(ObjetoReal(o), {
            has(t, k) {
              const h = tiene(t, k);
              if (h && !(typeof k === 'string' && empieza(k, PROPIO))) uso(`with ${typeof k === 'string' ? k : 'symbol'}`);
              return h;
            },
          });
        });
        def('__difuzzNeutroClave', (k) => (n ? `__dz_${typeof k === 'symbol' ? 'toPrimitive' : k}` : k));
      } catch {}
    }
    if (GANCHOS) {
      const sinUso = neutro === '1';
      const ramas = Object.create(null);
      cuentasDeRamas = ramas;
      try {
        def('__difuzzNeutroRama', (i) => {
          ramas[i] = (ramas[i] | 0) + 1;
        });
        def('__difuzzNeutroUso', (k) => {
          if (!sinUso) uso(`coercion ${k}`);
        });
        def('__difuzzNeutroValor', (v, k) => {
          if (sinUso || typeof v !== 'function') return v;
          return function () {
            uso(`coercion ${k}`);
            return new.target ? construir(v, arguments, new.target) : aplicar(v, this, arguments);
          };
        });
        def('__difuzzNeutroThis', (t) => {
          if (t === undefined || t === null || (typeof t !== 'object' && typeof t !== 'function')) uso('sloppy this');
        });
      } catch {}
    }
  }

  // Globals que crea el programa (asignaciones implicitas en sloppy): el minifier no
  // puede renombrarlos, y observarlos al salir no agrega usos al fuente.
  const baseGlobals = new Set(Reflect.ownKeys(globalThis));
  const observeNewGlobals = () => {
    prenderSiPisado();
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
  // b7-1b (revision 7.1, H2 d04): DIFUZZ_BUILTINS_USO=<lo mismo que FIJOS> (el camino de B con lo
  // pisado libre, recheck::builtins_matter): cada propiedad pasa a un accessor configurable que
  // guarda lo que el programa escribe y anota `uso builtins <spec>` en cada lectura DESPUES de una
  // escritura (leer lo pisado es usar la asuncion, aunque B no cambie de camino: `[1].includes(2)`
  // con `includes = Boolean`). Una escritura sobre otro objeto que hereda la propiedad crea la
  // propiedad propia, como con un dato. Si el programa la redefine o la borra, o era un accessor,
  // no se puede sensar: `uso? builtins <spec>` (al salir, o al instalarlo).
  {
    const specs = process.env.DIFUZZ_BUILTINS_USO;
    delete process.env.DIFUZZ_BUILTINS_USO;
    if (specs && COBERTURA) {
      const instalados = [];
      for (const spec of specs.split(',')) {
        try {
          const [id, ks] = spec.split(' ');
          const o = objetoDe(id);
          const k = leerClave(ks);
          if (!o || k === undefined) throw new Error('sin objeto');
          const d = Object.getOwnPropertyDescriptor(o, k);
          if (d && !('value' in d)) throw new Error('accessor');
          if (d && !d.configurable) throw new Error('no configurable');
          let v = d ? d.value : undefined;
          let pisado = false;
          const get = function () {
            if (pisado) uso(`builtins ${spec}`);
            return v;
          };
          const set = function (x) {
            if (this !== o) {
              if (this !== null && (typeof this === 'object' || typeof this === 'function')) {
                definir(this, k, { value: x, writable: true, enumerable: true, configurable: true });
              }
              return;
            }
            v = x;
            pisado = true;
          };
          Object.defineProperty(o, k, { get, set, enumerable: d ? d.enumerable : false, configurable: true });
          instalados.push([o, k, get, spec]);
        } catch {
          usoNoSensado(`builtins ${spec}`);
        }
      }
      process.on('exit', () => {
        for (let i = 0; i < instalados.length; i++) {
          const x = instalados[i];
          try {
            const d = descriptorPropio(x[0], x[1]);
            if (!d || d.get !== x[2]) usoNoSensado(`builtins ${x[3]}`);
          } catch {
            usoNoSensado(`builtins ${x[3]}`);
          }
        }
      });
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
    // 7.5: los archivos de los marcos que guardo prepareStackTrace.
    try {
      for (let s = 0; s < nStacksDeMarco; s++) {
        const lineas = stacksDeMarco[s];
        for (let i = 0; i < lineas.length; i++) anotarArchivosDeMarco(lineas[i]);
      }
    } catch {}
    for (const a in archivosDeMarco) ctl(`marco ${a}`);
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
    prenderSiPisado();
    try {
      return ser(v, 0, new Set());
    } catch (e) {
      return `[throws ${thrownType(e)}]`;
    }
  };
  const invocar = (name, f) => {
    let r;
    // t-sensor: lo que lanza `f` lo atrapa este try: desde aca el sensor tiene que estar.
    prenderPerezoso();
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
      notarSandbox(e);
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
      prenderSiPisado();
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

    // 8.4 (= 7.12): con --permission, el `require` de un DIRECTORIO que existe (`'..'`, `'../'`,
    // `'.'`, `'../..'`, `'/etc'`) lee su `package.json`, fuera de lo permitido, y node lanza
    // ERR_ACCESS_DENIED desde la busqueda (`Module._findPath` -> tryPackage ->
    // package_json_reader, MEDIDO node 24.14.1) donde sin el sandbox da MODULE_NOT_FOUND. Para
    // el sandbox, un package.json de afuera es uno que no existe: `_findPath` devuelve `false`
    // (no encontro) y node arma su MODULE_NOT_FOUND (mensaje con `Require stack`,
    // `requireStack`), que va al enlace (71) o, con --stubs, al stub, igual que sin el sandbox.
    // No se lee nada: lo negado sigue negado. Cubre lo que resuelve por CJS (`require`,
    // `require.resolve`, `createRequire` y el resolve por defecto de los hooks de --stubs, que
    // llaman a `Module._findPath` por la propiedad; MEDIDO). Solo esa lectura: cargar un archivo
    // de afuera que existe (`require('/etc/hostname')`) se niega al cargar, fuera de `_findPath`,
    // y sigue tocando el sandbox. ESM no lo necesita: `import` de un directorio da
    // ERR_UNSUPPORTED_DIR_IMPORT con y sin el sandbox (MEDIDO). Con el sensor apagado mientras
    // busca: si no, pausaria en el ERR_ACCESS_DENIED atrapado aca y marcaria `sandbox`. Mismos
    // atributos y `length` (3) que el de node.
    if (conPermisos) {
      try {
        const Module = require('module');
        const desc = descriptorPropio(Module, '_findPath');
        const real = desc && desc.value;
        const aplicar = Reflect.apply;
        // Sin getters ni Proxy: `code`, `permission` y `resource` son datos propios del error
        // de node (MEDIDO).
        const pkgNegado = (e) => {
          try {
            if (e === null || typeof e !== 'object' || esProxy(e)) return false;
            const c = descriptorPropio(e, 'code');
            const p = descriptorPropio(e, 'permission');
            const r = descriptorPropio(e, 'resource');
            return !!c && c.value === 'ERR_ACCESS_DENIED' && !!p && p.value === 'FileSystemRead' &&
              !!r && typeof r.value === 'string' && r.value.endsWith('/package.json');
          } catch {
            return false;
          }
        };
        if (typeof real === 'function') {
          const envuelto = {
            _findPath(request, paths, isMain) {
              const args = arguments;
              return sinSensor(() => {
                try {
                  return aplicar(real, this, args);
                } catch (e) {
                  if (pkgNegado(e)) return false;
                  throw e;
                }
              });
            },
          }._findPath;
          // El de node no tiene nombre (`Module._findPath = function (...)`, MEDIDO: `name` '').
          const nombre = descriptorPropio(real, 'name');
          if (nombre) Object.defineProperty(envuelto, 'name', nombre);
          Object.defineProperty(Module, '_findPath', { ...desc, value: envuelto });
        }
      } catch {}
    }

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
            return sinSensor(() => {
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
            });
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
  //     t-sensor (MEDIDO, node 24): Debugger.enable es casi todo el costo fijo (~11 ms dentro
  //     del proceso, 17-27 ms de pared; require+connect ~1,6 ms, el resto ~0); ~0,5 ms por pausa.
  // Igual en A y en B: lo que cambie, cambia en los dos.
  // DIFUZZ_SENSOR=0: sin sensor (runner.rs lo pide si node murio por una senal con el).
  // DIFUZZ_SENSOR=perezoso (t-sensor): el programa no puede atrapar (runner::puede_atrapar);
  // el sensor se prende solo si el preload va a atrapar algo del programa (`invocar`).
  const modoSensor = process.env.DIFUZZ_SENSOR;
  delete process.env.DIFUZZ_SENSOR;
  // Integracion 7.1 + 7.4: DIFUZZ_MAX_PAUSAS (solo la pone recheck::camino_sensor_i3, para correr
  // B con el sensor cuando hace falta su camino tras I3) cambia MAX_PAUSAS. Se lee aca, antes
  // del programa (el sensor perezoso se prende despues).
  const maxPausasEnv = process.env.DIFUZZ_MAX_PAUSAS;
  delete process.env.DIFUZZ_MAX_PAUSAS;
  const MAX_PAUSAS_ENV = typeof maxPausasEnv === 'string' && /^[1-9][0-9]{0,6}$/.test(maxPausasEnv) ? +maxPausasEnv : 256;
  const prenderSensor = () => { try {
    const PRELOAD_URL = require('url').pathToFileURL(__filename).href;
    const inspector = require('inspector');
    const sesion = new inspector.Session();
    sesion.connect();
    const MAX_PAUSAS = MAX_PAUSAS_ENV;
    let pausas = 0;
    const DESC = [
      ['tdz', /^ReferenceError: [^\n]* before initialization/],
      ['symbol', /^TypeError: Cannot convert a Symbol value to a/],
      ['extends', /^TypeError: Class extends value/],
    ];
    // n-sandbox: el comienzo de la descripcion (el stack) de un error que el sandbox provoco.
    // La del SystemError de `os.setPriority` llega sin el `[ERR_SYSTEM_ERROR]` (MEDIDO node
    // 24.14.1: `SystemError: A system error occurred: uv_os_setpriority returned EPERM ...`).
    const SANDBOX_DESC =
      /^(Error: (Access to this API has been restricted|kill EPERM)|SystemError( \[ERR_SYSTEM_ERROR\])?: A system error occurred: uv_os_setpriority returned EPERM)/;
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
        // n-sandbox: un permiso denegado que el programa atrapa (lo lanza node desde adentro:
        // el marco de arriba no es file:, por eso va antes del filtro de url). 2b: y una senal
        // a otro proceso que el filtro seccomp nego (`Error: kill EPERM`, MEDIDO node 24.14.1).
        // 7.7: y `os.setPriority` a otro proceso, negado por el mismo filtro.
        if (conPermisos && (p.reason === 'exception' || p.reason === 'promiseRejection') && SANDBOX_DESC.test(desc)) ctl('sandbox');
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
  } catch {} };
  if (modoSensor === 'perezoso') sensorPerezoso = prenderSensor;
  else if (modoSensor !== '0') prenderSensor();

  // --- b7-1 (revision n2, IMPORTANTE 1): cobertura de bloques -----------------------
  // DIFUZZ_COBERTURA=1: cobertura precisa de V8 (por bloque y con cuentas) desde antes de
  // compilar el programa; al salir, `cobertura <hash> <rangos>` en el canal. El hash es de
  // los scripts del programa (file: menos este preload, y los de eval, sin url), cada uno
  // con sus funciones y sus rangos (inicio, fin, cuenta) en el orden de V8: dos corridas
  // del MISMO texto dan el mismo hash si y solo si (salvo colision) pasaron por los mismos
  // bloques la misma cantidad de veces. Lo usan los re-chequeos para ver si B cambia de
  // camino con la asuncion neutralizada (recheck::comparar_caminos). Va al final: el
  // listener de `exit` corre despues de los del preload (observar al salir puede llamar
  // codigo del programa); los que registre el programa corren despues y no cuentan.
  // `takePreciseCoverage` contesta sincronico en el hilo principal (MEDIDO, node 24.14.1,
  // con salida normal, process.exit y throw no atrapado).
  // b7-1b (revision 7.1, H4): los scripts de `vm` (url `evalmachine.<anonymous>` o el `filename`
  // que se le da) tambien cuentan; afuera quedan este preload y los de node (`node:`). Y las
  // cuentas de `__difuzzNeutroRama` (los ganchos de arriba) entran al hash.
  //
  // b7-1b (revision 7.1, H5): con --permission y --allow-inspector (el sensor y esta cobertura
  // usan una sesion del inspector en el proceso), el programa podria abrir un puerto del
  // inspector (MEDIDO, node 24.14.1: `inspector.open`, `inspector/promises.open`,
  // `process._debugProcess` y SIGUSR1). Los dos ultimos los corta `--disable-sigusr1`
  // (runner::permisos_node, MEDIDO); los dos primeros, esto: `open` niega como el modelo de
  // permisos (ERR_ACCESS_DENIED) y anota `sandbox`, como un permiso negado. Las sesiones en el
  // proceso (`inspector.Session`) siguen: es lo que usa el preload.
  try {
    if (conPermisos && process.permission.has('inspector')) {
      const negar = () => {
        ctl('sandbox');
        const e = new Error('Access to this API has been restricted (difuzz: inspector.open)');
        e.code = 'ERR_ACCESS_DENIED';
        throw e;
      };
      for (const m of ['inspector', 'inspector/promises']) {
        Object.defineProperty(require(m), 'open', { value: negar, writable: false, configurable: false, enumerable: true });
      }
    }
  } catch {}
  if (COBERTURA) {
    try {
      const PRELOAD_URL = require('url').pathToFileURL(__filename).href;
      const sesion = new (require('inspector').Session)();
      const postar = desligar(sesion.post);
      sesion.connect();
      sesion.post('Profiler.enable');
      sesion.post('Profiler.startPreciseCoverage', { callCount: true, detailed: true });
      process.on('exit', () => {
        try {
          postar(sesion, 'Profiler.takePreciseCoverage', (err, res) => {
            if (err || !res || !esArreglo(res.result)) return;
            // Dos FNV-1a de 32 bits con distinta semilla (64 bits en total).
            // b7-1c (S2): con lo tomado al principio y con indices (sin iteradores).
            let h1 = 0x811c9dc5, h2 = 0x01000193 ^ 0x5bd1e995, rangos = 0;
            const mezclar = (s) => {
              for (let i = 0; i < s.length; i++) {
                const c = strCharCodeAt(s, i);
                h1 = imul(h1 ^ c, 0x01000193);
                h2 = imul(h2 ^ c, 0x01000193 + 2);
              }
            };
            const scripts = res.result;
            for (let a = 0; a < scripts.length; a++) {
              const sc = scripts[a];
              const url = typeof sc.url === 'string' ? sc.url : '';
              if (url === PRELOAD_URL || strEmpiezaEn(url, 'node:') || strEmpiezaEn(url, 'difuzz-stub:')) continue;
              mezclar(`s${url}|`);
              const fns = esArreglo(sc.functions) ? sc.functions : [];
              for (let b = 0; b < fns.length; b++) {
                mezclar('f');
                const rs = esArreglo(fns[b].ranges) ? fns[b].ranges : [];
                for (let c = 0; c < rs.length; c++) {
                  const r = rs[c];
                  rangos++;
                  mezclar(`${r.startOffset},${r.endOffset},${r.count};`);
                }
              }
            }
            if (cuentasDeRamas !== null) {
              mezclar('r');
              const ks = clavesPropias(cuentasDeRamas);
              for (let i = 0; i < ks.length; i++) mezclar(`${ks[i]}:${cuentasDeRamas[ks[i]]};`);
            }
            // Sin `Number.prototype.toString` ni `padStart` (el programa los puede pisar).
            const HEX = '0123456789abcdef';
            const hex = (x) => {
              let t = '';
              for (let i = 28; i >= 0; i -= 4) t += HEX[(x >>> i) & 15];
              return t;
            };
            ctl(`cobertura ${hex(h1)}${hex(h2)} ${rangos}`);
          });
        } catch {}
      });
    } catch {}
  }
})();
