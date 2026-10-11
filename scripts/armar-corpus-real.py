#!/usr/bin/env python3
"""Arma un corpus de difuzz (`NNNN/input.js`) con archivos JS reales que ya estan en el disco.

Solo lee las fuentes. Determinista: mismo disco + misma semilla => mismo corpus.

Fuentes:
  --dir D        directorios con node_modules u otros .js/.cjs/.mjs (repetible)
  --cacache C    la cache de npm (~/.npm/_cacache): lee los tarballs del index-v5 sin extraerlos

Criterios (cada rechazo se cuenta por causa en <out>/stats.tsv):
  - extension .js/.cjs/.mjs; nada de .min.js; tamano entre --min y --max bytes
  - UTF-8 valido; no parece minificado (linea media <= 200 y ninguna linea > 2000 chars)
  - sin capacidades peligrosas: fs, child_process, net, http(s), dgram, tls, worker_threads,
    cluster, inspector, vm, process.binding/dlopen/kill/chdir, addons .node, fetch, etc.
    difuzz corre node sin --permission por defecto (scripts/barrido-real.sh fija --sandbox-red):
    el filtro es una barrera mas, heuristica, por texto.
    --neutralizar si|solo: en vez de descartar, reescribe esos especificadores a
    `difuzz-neutralizado-<mod>` (no resuelve; con difuzz --stubs es un stub inerte).
  - sin require/import dinamico (require(variable), createRequire, getBuiltinModule...):
    podria traer un modulo peligroso por un camino que el filtro no ve.
  - forma: --forma cjs (sin import/export de modulo) o esm (.mjs o con import/export top-level)
  - requires: --requires ninguno (solo autocontenidos, a lo sumo builtins inocuos),
    externos (cualquier especificador; para correr con --stubs)
  - --minificados acepta .min.js y archivos con lineas largas (por defecto se descartan)
  - dedup por sha256 del contenido (despues de quitar el shebang)
  - pre-filtro de sintaxis (D2, 10-oct): lo que node no compila (Flow, JSX, TypeScript, un
    `import` en un archivo que es CJS...) no entra al corpus. Un solo proceso de node para todo
    el lote (no un node por archivo) que COMPILA sin ejecutar: CJS con vm.compileFunction y los
    parametros del envoltorio de modulos (como el carril A, .cjs), ESM con vm.SourceTextModule
    (como el carril B, .mjs). Esos archivos quedan en fuentes.tsv con caso "-" y el motivo, y se
    cuentan en stats.tsv como rechazo_sintaxis. --node elige el node (default: el del PATH)
Muestra: --n casos al azar con random.Random(--semilla) sobre la lista ordenada por hash (despues
del pre-filtro: los rechazados no ocupan lugar en la muestra).
Salida: <out>/NNNN/input.js, <out>/fuentes.tsv (caso, sha256, bytes, forma, n_requires, origen,
estado: "ok" o "sintaxis: <mensaje de node>"), <out>/stats.tsv (rechazos por causa). Las licencias
de los archivos copiados son de sus autores: el corpus no se versiona.

No hace red: solo lee las fuentes que se le pasan. Para correr el corpus: scripts/barrido-real.sh
(fija --sandbox-red).

Ejemplo (las fuentes son las de cada maquina):
  scripts/armar-corpus-real.py --cacache ~/.npm/_cacache --dir OTRO/node_modules \\
      --forma cjs --requires externos --neutralizar si --n 100000 --out CORPUS
"""
import argparse
import hashlib
import io
import json
import os
import random
import re
import subprocess
import sys
import tarfile
import zlib

EXTS = ('.js', '.cjs', '.mjs')
PELIGROSOS = (
    'fs', 'fs/promises', 'child_process', 'net', 'http', 'https', 'http2', 'dgram', 'tls',
    'cluster', 'worker_threads', 'inspector', 'inspector/promises', 'vm', 'v8', 'repl',
    'readline', 'readline/promises', 'module', 'dns', 'dns/promises', 'process', 'wasi',
    'trace_events', 'diagnostics_channel', 'async_hooks', 'perf_hooks', 'tty', 'sqlite', 'test',
)
INOCUOS = (
    'util', 'path', 'events', 'assert', 'assert/strict', 'buffer', 'string_decoder', 'url',
    'querystring', 'punycode', 'os', 'stream', 'stream/promises', 'zlib', 'crypto', 'timers',
    'timers/promises', 'constants', 'util/types', 'path/posix', 'path/win32', 'stream/web',
)
RE_SPEC = re.compile(
    r"""(?:\brequire\s*\(\s*|\bimport\s*\(\s*|\bfrom\s*|^\s*import\s*)(['"])([^'"\n]{1,200})\1""",
    re.M,
)
RE_ESM = re.compile(r'^\s*(?:import\s*[\w{*\'"]|export\s+(?:default|const|let|var|function|class|async|\{|\*))', re.M)
RE_PELIGRO_TXT = re.compile(
    r'process\s*\.\s*(?:binding|_linkedBinding|dlopen|kill|chdir|setuid|setgid|abort)\b'
    r'|\.node[\'"]|\bprocess\.mainModule|\bglobalThis\.process\.binding|\bWebSocket\b|\bfetch\s*\('
    r'|\bXMLHttpRequest\b|\bDeno\.|\bBun\.'
)


RE_DINAMICO = re.compile(
    r'\brequire\s*\(\s*[^\s\'")]|\bimport\s*\(\s*[^\s\'")]|module\.constructor|createRequire|getBuiltinModule'
    r'|\brequire\s*\[|\brequire\s*\)|=\s*require\b\s*[,;]'
)


# Pre-filtro de sintaxis: lee lineas JSON {"esm": bool, "src": str} por stdin y escribe, por cada
# una y en el mismo orden, "ok" o el mensaje del SyntaxError (en una linea). Solo compila: ni
# compileFunction ni el constructor de SourceTextModule evaluan el codigo (no hay link/evaluate).
CHEQUEO_JS = r"""
'use strict';
const vm = require('vm');
const PARAMS = ['exports', 'require', 'module', '__filename', '__dirname'];
// Por lineas y sin juntar todo stdin en un string: un lote de archivos grandes puede pasar el
// largo maximo de un string de V8.
const rl = require('readline').createInterface({ input: process.stdin, crlfDelay: Infinity });
const out = [];
rl.on('line', (l) => {
  if (!l) return;
  const { esm, src } = JSON.parse(l);
  try {
    if (esm) new vm.SourceTextModule(src);
    else vm.compileFunction(src, PARAMS);
    out.push('ok');
  } catch (e) {
    let m;
    try { m = `${e && e.name}: ${e && e.message}`; } catch { m = 'error'; }
    out.push(m.replace(/[\t\r\n]+/g, ' ').slice(0, 200));
  }
});
rl.on('close', () => process.stdout.write(out.join('\n') + '\n'));
"""


def chequear_sintaxis(node, entradas):
    """entradas: lista de (txt, forma). Devuelve una lista de "ok" o el motivo, en el mismo orden.
    Un solo proceso de node para todo el lote."""
    if not entradas:
        return []
    datos = ''.join(json.dumps({'esm': forma == 'esm', 'src': txt}) + '\n' for txt, forma in entradas)
    r = subprocess.run(
        [node, '--no-warnings', '--experimental-vm-modules', '-e', CHEQUEO_JS],
        input=datos.encode('utf-8'), capture_output=True, check=False,
    )
    res = r.stdout.decode('utf-8').split('\n')[:-1]
    if r.returncode != 0 or len(res) != len(entradas):
        sys.exit(f'el pre-filtro de sintaxis fallo (node rc {r.returncode}, {len(res)} de '
                 f'{len(entradas)} respuestas): {r.stderr.decode("utf-8", "replace")[-2000:]}')
    return res


def norm_spec(s):
    return s[5:] if s.startswith('node:') else s


def clasificar(nombre, data, a):
    """Devuelve (causa_rechazo | None, info)."""
    base = os.path.basename(nombre)
    if not base.endswith(EXTS):
        return 'extension', None
    if '.min.' in base and not a.minificados:
        return 'min_js', None
    if len(data) < a.min:
        return 'chico', None
    if len(data) > a.max:
        return 'grande', None
    try:
        txt = data.decode('utf-8')
    except UnicodeDecodeError:
        return 'no_utf8', None
    if txt.startswith('#!'):
        nl = txt.find('\n')
        txt = '' if nl < 0 else txt[nl + 1:]
    lineas = txt.split('\n')
    if not a.minificados and (max(len(l) for l in lineas) > 2000 or len(txt) / max(1, len(lineas)) > 200):
        return 'minificado', None
    peligroso = lambda s: s in PELIGROSOS or s.split('/')[0] in ('fs', 'child_process', 'net', 'http', 'https')
    specs = [norm_spec(m.group(2)) for m in RE_SPEC.finditer(txt)]
    if any(peligroso(s) for s in specs):
        if not a.neutralizar:
            return 'peligroso_modulo', None
        # --neutralizar: el especificador peligroso pasa a uno que no resuelve, y difuzz --stubs lo
        # reemplaza por un stub inerte. No cubre require(variable) ni process.mainModule (texto).
        txt = RE_SPEC.sub(
            lambda m: m.group(0)[: m.start(2) - m.start(0)] + 'difuzz-neutralizado-' + norm_spec(m.group(2)).replace('/', '-')
            + m.group(0)[m.end(2) - m.start(0):] if peligroso(norm_spec(m.group(2))) else m.group(0),
            txt,
        )
        specs = [norm_spec(m.group(2)) for m in RE_SPEC.finditer(txt)]
        if any(peligroso(s) for s in specs):
            return 'peligroso_modulo', None
    elif a.neutralizar == 'solo':
        return 'no_neutralizado', None
    if RE_PELIGRO_TXT.search(txt):
        return 'peligroso_texto', None
    # Un require(variable) podria traer un modulo peligroso por un camino que el filtro no ve.
    # (Agregado a las 20:05: los corpus cjs-auto, cjs-ext, esm-*, *-grande, *-enorme se armaron
    # antes y tienen 37 archivos asi; cjs-neutr ya lo aplicaba.)
    if RE_DINAMICO.search(txt):
        return 'peligroso_dinamico', None
    esm = base.endswith('.mjs') or (not base.endswith('.cjs') and bool(RE_ESM.search(txt)))
    forma = 'esm' if esm else 'cjs'
    if forma != a.forma:
        return 'forma_' + forma, None
    externos = [s for s in specs if s not in INOCUOS]
    if a.requires == 'ninguno' and externos:
        return 'requires', None
    if '\x00' in txt:
        return 'nul', None
    return None, (txt, forma, len(externos))


def iter_dirs(dirs):
    for d in dirs:
        for raiz, subdirs, archivos in os.walk(d, followlinks=False):
            subdirs.sort()
            for f in sorted(archivos):
                if f.endswith(EXTS):
                    p = os.path.join(raiz, f)
                    try:
                        if os.path.islink(p) or os.path.getsize(p) > 5_000_000:
                            continue
                        with open(p, 'rb') as fh:
                            yield p, fh.read()
                    except OSError:
                        continue


def tarballs(cacache):
    idx = os.path.join(cacache, 'index-v5')
    vistos = {}
    for raiz, subdirs, archivos in os.walk(idx):
        subdirs.sort()
        for f in sorted(archivos):
            try:
                with open(os.path.join(raiz, f), encoding='utf-8', errors='replace') as fh:
                    for linea in fh:
                        tab = linea.find('\t')
                        if tab < 0:
                            continue
                        try:
                            e = json.loads(linea[tab + 1:])
                        except ValueError:
                            continue
                        k = e.get('key', '')
                        integ = e.get('integrity') or ''
                        if k.endswith('.tgz') and integ.startswith('sha512-'):
                            vistos[k] = integ
            except OSError:
                continue
    for k in sorted(vistos):
        import base64
        hx = base64.b64decode(vistos[k][7:]).hex()
        p = os.path.join(cacache, 'content-v2', 'sha512', hx[:2], hx[2:4], hx[4:])
        if os.path.exists(p):
            yield k.rsplit('/-/', 1)[-1], p


def iter_cacache(cacache, stats):
    for nombre, p in tarballs(cacache):
        stats['tarballs'] = stats.get('tarballs', 0) + 1
        try:
            with tarfile.open(p, 'r:gz') as t:
                for m in t:
                    if not m.isfile() or not m.name.endswith(EXTS) or m.size > 5_000_000:
                        continue
                    fh = t.extractfile(m)
                    if fh is None:
                        continue
                    yield f'npm:{nombre}:{m.name}', fh.read()
        except (tarfile.TarError, OSError, EOFError, zlib.error):
            stats['tarball_ilegible'] = stats.get('tarball_ilegible', 0) + 1


def main():
    ap = argparse.ArgumentParser(
        description=__doc__.split('\n\n', 1)[0],
        epilog=__doc__.split('\n\n', 1)[1],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument('--dir', action='append', default=[])
    ap.add_argument('--cacache')
    ap.add_argument('--out', required=True)
    ap.add_argument('--forma', choices=('cjs', 'esm'), default='cjs')
    ap.add_argument('--requires', choices=('ninguno', 'externos'), default='ninguno')
    ap.add_argument('--n', type=int, default=500)
    ap.add_argument('--semilla', type=int, default=1)
    ap.add_argument('--min', type=int, default=200)
    ap.add_argument('--max', type=int, default=30000)
    ap.add_argument('--minificados', action='store_true', help='aceptar archivos minificados y .min.js')
    ap.add_argument('--neutralizar', choices=('si', 'solo'), default=None,
                    help='reescribir require/import de fs, child_process, net, ... a un modulo inexistente (correr con --stubs); solo: ademas, solo los archivos que lo necesitaron')
    ap.add_argument('--excluir', default='', help='fuentes.tsv de un corpus previo: no repetir sus hashes')
    ap.add_argument('--node', default='node', help='node para el pre-filtro de sintaxis (default: el del PATH)')
    a = ap.parse_args()

    if os.path.exists(a.out):
        sys.exit(f'{a.out} ya existe')
    excluir = set()
    if a.excluir:
        with open(a.excluir, encoding='utf-8') as fh:
            for l in fh:
                c = l.split('\t')
                if len(c) > 1:
                    excluir.add(c[1])
    stats = {}
    pool = {}  # sha -> (zlib(txt), forma, n_ext, origen)
    vistos = 0

    def fuentes():
        yield from iter_dirs(a.dir)
        if a.cacache:
            yield from iter_cacache(a.cacache, stats)

    for origen, data in fuentes():
        vistos += 1
        causa, info = clasificar(origen, data, a)
        if causa:
            stats['rechazo_' + causa] = stats.get('rechazo_' + causa, 0) + 1
            continue
        txt, forma, nre = info
        h = hashlib.sha256(txt.encode('utf-8')).hexdigest()
        if h in pool:
            stats['duplicado'] = stats.get('duplicado', 0) + 1
            continue
        if h in excluir:
            stats['excluido_previo'] = stats.get('excluido_previo', 0) + 1
            continue
        pool[h] = (zlib.compress(txt.encode('utf-8')), forma, nre, origen)

    stats['archivos_vistos'] = vistos
    # Pre-filtro de sintaxis sobre todo el pool (antes de la muestra), en orden de hash.
    todas = sorted(pool)
    veredictos = chequear_sintaxis(
        a.node, [(zlib.decompress(pool[h][0]).decode('utf-8'), pool[h][1]) for h in todas])
    rechazados = {h: v for h, v in zip(todas, veredictos) if v != 'ok'}
    if rechazados:
        stats['rechazo_sintaxis'] = len(rechazados)
    stats['aceptados_unicos'] = len(pool) - len(rechazados)
    claves = [h for h in todas if h not in rechazados]
    rng = random.Random(a.semilla)
    elegidos = claves if a.n >= len(claves) else sorted(rng.sample(claves, a.n))
    rng.shuffle(elegidos)
    os.makedirs(a.out)
    ancho = max(4, len(str(len(elegidos))))
    with open(os.path.join(a.out, 'fuentes.tsv'), 'w', encoding='utf-8') as man:
        man.write('caso\tsha256\tbytes\tforma\tn_requires\torigen\testado\n')
        for i, h in enumerate(elegidos):
            z, forma, nre, origen = pool[h]
            txt = zlib.decompress(z)
            caso = str(i).zfill(ancho)
            os.makedirs(os.path.join(a.out, caso))
            with open(os.path.join(a.out, caso, 'input.js'), 'wb') as fh:
                fh.write(txt)
            man.write(f'{caso}\t{h}\t{len(txt)}\t{forma}\t{nre}\t{origen}\tok\n')
        # Los que el pre-filtro dejo afuera: caso "-" y el motivo (no entran al corpus).
        for h in todas:
            if h in rechazados:
                z, forma, nre, origen = pool[h]
                man.write(f'-\t{h}\t{len(zlib.decompress(z))}\t{forma}\t{nre}\t{origen}\tsintaxis: {rechazados[h]}\n')
    stats['muestra'] = len(elegidos)
    with open(os.path.join(a.out, 'stats.tsv'), 'w', encoding='utf-8') as fh:
        for k in sorted(stats):
            fh.write(f'{k}\t{stats[k]}\n')
    for k in sorted(stats):
        print(f'{k}\t{stats[k]}')


if __name__ == '__main__':
    main()
