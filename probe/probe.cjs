'use strict';
// Probe contra @swc/core: el producto que usa la gente, con opciones por defecto.
// Independiente de difuzz. Responde lo que difuzz no puede:
//   - ¿reproduce con defaults, o hace falta `toplevel`?
//   - ¿rompe dentro de una funcion? (la forma del codigo bundleado: wrappers, IIFE)
// Esa es la evidencia de "uso comun" que pide la policy de crates/swc_ecma_minifier/AGENTS.md.
//
// Uso: node probe.cjs archivo.js [archivo.js ...]    (exit 1 si algo diverge)

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const { minifySync } = require('@swc/core');

const CONFIGS = {
  defaults: { compress: {}, mangle: false },
  defaults_mangle: { compress: {}, mangle: {} },
  toplevel: { compress: {}, mangle: false, toplevel: true },
};
const FORMS = {
  tal_cual: (src) => src,
  en_funcion: (src) => `(function () {\n${src}\n})();\n`,
};

// Mismo contrato que el runner de difuzz: archivo .cjs, TZ=UTC, timeout.
const tmp = path.join(os.tmpdir(), `difuzz-probe-${process.pid}.cjs`);
const run = (code) => {
  fs.writeFileSync(tmp, code);
  const r = spawnSync(process.execPath, [tmp], {
    encoding: 'utf8',
    timeout: 5000,
    env: { ...process.env, TZ: 'UTC' },
  });
  if (r.error && r.error.code === 'ETIMEDOUT') return 'TIMEOUT';
  if (r.status === 0) return r.stdout.trim().replace(/\n/g, ' ⏎ ').slice(0, 200) || '(sin salida)';
  const m = r.stderr.match(/^\w*Error.*$/m);
  return m ? m[0] : `exit ${r.status}`;
};

let diverged = false;
for (const file of process.argv.slice(2)) {
  const raw = fs.readFileSync(file, 'utf8');
  console.log(`== ${file}`);
  for (const [fname, wrap] of Object.entries(FORMS)) {
    const src = wrap(raw);
    const a = run(src);
    for (const [cname, cfg] of Object.entries(CONFIGS)) {
      let code;
      try {
        code = minifySync(src, cfg).code;
      } catch (e) {
        console.log(`  ! ${fname.padEnd(10)} ${cname.padEnd(15)} MINIFY ERROR: ${e.message.split('\n')[0]}`);
        continue;
      }
      const b = run(code);
      const same = a === b;
      if (!same) diverged = true;
      console.log(`  ${same ? ' ' : '✗'} ${fname.padEnd(10)} ${cname.padEnd(15)} ${same ? a : `${a}  →  ${b}`}`);
    }
  }
}
try { fs.unlinkSync(tmp); } catch {}
process.exit(diverged ? 1 : 0);
