//! 9c: la clasificacion completa de un caso, de punta a punta, por el camino
//! real: `process_case` (y de ahi `compare_base`, `compare_and_report`,
//! `clasificar` y el `Reporter`) con la CLI parseada de verdad, el `Runner` y
//! node. Los backends son controlados: `Identidad` (parsea, imprime y corre los
//! chequeos estaticos reales de checks.rs), `Rompe` (lo mismo con un reemplazo
//! a proposito en el minificado), uno que hace panic, el oraculo de R1-H05 y el
//! hijo minificador real de subproceso.rs que no contesta a tiempo.
//!
//! Una tabla: programa -> destino (div/, known/, panic/, asserts/, recursos/,
//! lint/ o descarte) -> razon. Cada caso tiene que terminar en exactamente su
//! destino (y en ningun otro) con la razon que corresponde.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::backend::identidad::Identidad;
use crate::backend::{Backend, Minifier};
use crate::minify::{Config, Outcome, SourceKind};

/// Lo de `Identidad` con `de` reemplazado por `a` en el minificado. Los chequeos
/// estaticos y las escrituras muertas se recalculan sobre los tres textos con
/// el analisis real (`checks_on_text`), como hace un backend de verdad.
struct Rompe {
    de: &'static str,
    a: &'static str,
}

impl Backend for Rompe {
    fn prepare(&self, src: &str) -> Outcome {
        match (Identidad { kind: SourceKind::Script }).prepare(src) {
            Outcome::Ok { original, minified, .. } => {
                assert!(minified.contains(self.de), "Rompe: {:?} no esta en {minified:?}", self.de);
                let minified = minified.replacen(self.de, self.a, 1);
                let (static_flags, dead_writes, analisis) = crate::backend::swc::checks_on_text(src, &original, &minified, SourceKind::Script, Minifier::Identidad);
                Outcome::Ok { original, minified, static_flags, dead_writes, analisis }
            }
            o => o,
        }
    }
}

/// Un minifier que hace panic con cualquier fuente.
struct Panica;

impl Backend for Panica {
    fn prepare(&self, _: &str) -> Outcome {
        panic!("e2e: panic simulado del minifier")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destino {
    Div,
    Known,
    Panic,
    Asserts,
    Recursos,
    Lint,
    /// Con su causa en `descartes.tsv`.
    Descarte(&'static str),
    /// Control: iguales, no se escribe nada.
    Ninguno,
}

impl Destino {
    fn dir(self) -> Option<&'static str> {
        match self {
            Destino::Div => Some("div"),
            Destino::Known => Some("known"),
            Destino::Panic => Some("panic"),
            Destino::Asserts => Some("asserts"),
            Destino::Recursos => Some("recursos"),
            Destino::Lint => Some("lint"),
            Destino::Descarte(_) | Destino::Ninguno => None,
        }
    }
}

/// Que backend usa la fila.
enum Back {
    Identidad,
    Rompe(&'static str, &'static str),
    Panica,
    /// Panic con debug assertions y el oraculo (Identidad) minifica.
    PanicaConOraculo,
    /// El hijo minificador real (subproceso.rs) que duerme mas que su timeout.
    HijoLento,
    /// M5 (revision-b456): el backend de la celda de verdad (`--minifier`,
    /// `--config` de la fila), con `Ctx::medido` como en produccion: corren los
    /// re-chequeos de su politica (los backends falsos no se pueden re-minificar).
    Celda,
}

impl Back {
    fn config(&self, cli: &crate::Cli) -> Config {
        let id = || Box::new(Identidad { kind: SourceKind::Script });
        match *self {
            Back::Celda => crate::configs_de_celda(cli).0,
            Back::Identidad => Config::from_backend(id()),
            Back::Rompe(de, a) => Config::from_backend(Box::new(Rompe { de, a })),
            Back::Panica => Config::from_backend(Box::new(Panica)),
            Back::PanicaConOraculo => Config::con_oraculo(Box::new(Panica), id()),
            Back::HijoLento => Config::from_backend(Box::new(crate::subproceso::tests::remoto_de_test_con(
                Minifier::Swc,
                SourceKind::Script,
                &[("DIFUZZ_TEST_DORMIR_MS", "20000")],
                Duration::from_millis(300),
                Some(0),
            ))),
        }
    }
}

struct Fila {
    caso: &'static str,
    programa: &'static str,
    back: Back,
    /// Flags de la CLI ademas de los comunes.
    args: &'static [&'static str],
    destino: Destino,
    /// Lo que tiene que decir el meta.txt del destino (todas las lineas).
    meta: &'static [&'static str],
}

/// La traza de `clasificar` de una divergencia sin asuncion (sin re-chequeos
/// propios del backend: `Ctx::medido` es `None`).
const TRAZA_DIV: &str = "\nclasificacion:\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: ninguna (ni flag de la tabla, ni ruta, ni backlog)\n  decision: div/\n";

const TRAZA_COERCION: &str = "\nclasificacion:\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: sensor, asuncion: oxc: Coercion Methods Are Pure\n  minificado_invalido: no (node --check acepta el minificado)\n  sintoma: el de known-sintomas.tsv (o primero registrado)\n  decision: known/ (sensor, asuncion: oxc: Coercion Methods Are Pure)\n";

const TRAZA_BUILTINS: &str = "\nclasificacion:\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: sensor, asuncion: oxc: No Monkey-Patching Built-ins\n  minificado_invalido: no (node --check acepta el minificado)\n  sintoma: el de known-sintomas.tsv (o primero registrado)\n  decision: known/ (sensor, asuncion: oxc: No Monkey-Patching Built-ins)\n";

/// b4-a / M5: la TDZ atrapada que oxc borra, por el camino de produccion (oxc
/// core_defaults, re-chequeo de violaciones Explica; MEDIDA). `contenido` sigue
/// dependiendo de que V8 formatea `.stack` al pausar (riesgo documentado de b4-a).
const TRAZA_TDZ_ATRAPADA: &str = "\nclasificacion:\n  contenido: NoExplica -> contenido_irrelevante (A con toString/.stack cambiados da lo mismo)\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  nombres: no aplica (la salida no puede mostrar nombres)\n  violaciones: Explica (quedan tdz_atrapada)\n  asunciones: sensor, asuncion: oxc: No TDZ Violation (atrapada)\n  minificado_invalido: no (node --check acepta el minificado)\n  sintoma: el de known-sintomas.tsv (o primero registrado)\n  decision: known/ (sensor, asuncion: oxc: No TDZ Violation (atrapada))\n";

const TRAZA_CAUSA: &str = "\nclasificacion:\n  contenido: Explica (A depende del contenido y con el contenido fijo la divergencia desaparece)\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: sensor, asuncion: difuzz: el contenido de Error.prototype.stack cambia al minificar (oxc no lo documenta)\n  minificado_invalido: no (node --check acepta el minificado)\n  sintoma: el de known-sintomas.tsv (o primero registrado)\n  decision: known/ (sensor, asuncion: difuzz: el contenido de Error.prototype.stack cambia al minificar (oxc no lo documenta))\n";

const TRAZA_INVALIDO: &str = "\nclasificacion:\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: sensor, asuncion: oxc: Coercion Methods Are Pure\n  minificado_invalido: si (node --check rechaza el minificado y acepta el original): sin flags, ninguna\n  decision: div/\n";

fn tabla() -> Vec<Fila> {
    vec![
        // Control: el minificado es el original; ni siquiera corre B.
        Fila { caso: "igual", programa: "console.log(41);\n", back: Back::Identidad, args: &[], destino: Destino::Ninguno, meta: &[] },
        // Un bug: imprime otra cosa y nada lo explica.
        Fila {
            caso: "div-stdout",
            programa: "console.log(41);\n",
            back: Back::Rompe("41", "42"),
            args: &[],
            destino: Destino::Div,
            meta: &["kind: Stdout\n", "\nobserved: false\n", "\nknown: no\n", "\nnota: -\n", "\nflags_a: -\n", TRAZA_DIV],
        },
        // Un bug en un programa mudo: lo ve el epilogo observador (segunda pasada).
        Fila {
            caso: "div-observado",
            programa: "var k = 41;\n",
            back: Back::Rompe("41", "42"),
            args: &[],
            destino: Destino::Div,
            meta: &["kind: Stdout\n", "\nobserved: true\n", "\nknown: no\n", TRAZA_DIV],
        },
        // Flag estatico real (checks.rs: `coercion`) -> asuncion de la tabla de oxc.
        Fila {
            caso: "known-estatico",
            programa: "var o = { valueOf() { return 1; } };\nconsole.log(41);\n",
            back: Back::Rompe("41", "42"),
            args: &[],
            destino: Destino::Known,
            meta: &["\nknown: sensor, asuncion: oxc: Coercion Methods Are Pure\n", "\nflags_a: coercion\n", TRAZA_COERCION],
        },
        // Flag de runtime real (preload.js: `builtins`).
        Fila {
            caso: "known-runtime",
            programa: "Array.prototype.extra = 1;\nconsole.log(41);\n",
            back: Back::Rompe("41", "42"),
            args: &[],
            destino: Destino::Known,
            meta: &["\nknown: sensor, asuncion: oxc: No Monkey-Patching Built-ins\n", "\nflags_a: builtins\n", TRAZA_BUILTINS],
        },
        // b4-a (R1-H04 / C3): flag del sensor de violaciones atrapadas (preload.js,
        // inspector) -> asuncion de la tabla de oxc. M5 (revision-b456): por el camino de
        // produccion (oxc de verdad, `Ctx::medido` y el runner de la celda, con el sensor
        // por la politica de oxc), asi corre el re-chequeo de violaciones. Antes: `Rompe`
        // y `medido = None` (sin re-chequeo) y `flags_b: stack,...`, un detalle de V8.
        Fila {
            caso: "known-violacion",
            programa: "try { x; console.log(\"sin tdz\"); } catch (e) { console.log(\"tdz\"); }\nlet x = 1;\n",
            back: Back::Celda,
            args: &["--minifier", "oxc", "--config", "core_defaults"],
            destino: Destino::Known,
            meta: &["\nknown: sensor, asuncion: oxc: No TDZ Violation (atrapada)\n", TRAZA_TDZ_ATRAPADA],
        },
        // R2-H07 (b6-usuario): `coercion` (estatico, sin re-chequeo) y `stack`, que el
        // re-chequeo de contenido confirma: la razon es la de `stack`, no la del primer
        // flag de la tabla (antes: "Coercion Methods Are Pure").
        // t-principio: antes el "bug" de Rompe (41 -> 42) iba en la misma linea que el largo
        // del stack y el paso contenido lo aceptaba; ahora A' y B' con el contenido fijo lo
        // ven (NoExplica).
        // n-ruido (t-real): un stack que solo cambia de columna al minificar no diverge.
        Fila {
            caso: "stack-columna",
            programa: "var o = { valueOf() { return 1; } };\nconsole.log(new Error('e').stack);\n",
            back: Back::Rompe("console.log(new", "console.log( new"),
            args: &[],
            destino: Destino::Ninguno,
            meta: &[],
        },
        // b7-3 (revision n2, MENOR 4): lo que es reloj sigue oculto. El minificado gasta
        // ~30 ms antes de armar los timers (sin salida ni globals): `_idleStart`, la
        // TimersList (`expiry`) y los vecinos cambian, y A y B dan lo mismo, con los timers
        // vivos (los observa el epilogo) y cancelados (los globals, al salir).
        Fila {
            caso: "timer-reloj",
            programa: "var t = setTimeout(function () {}, 50), u = setInterval(function () {}, 20), w = setTimeout(function () {}, 50);\nvar i = setImmediate(function () {}, 1);\nsetTimeout(function () { clearTimeout(t); clearInterval(u); clearTimeout(w); clearImmediate(i); }, 1);\n",
            back: Back::Rompe("var t=", "(function(){for(var q=0;q<3e7;q++);})();var t="),
            args: &[],
            destino: Destino::Ninguno,
            meta: &[],
        },
        // b7-3: un timer que pasa de setTimeout a setInterval solo se ve en `_repeat` (el
        // callback no corre: se cancela). Con los timers opacos de n-ruido no divergia; ahora
        // el preload imprime `_repeat`.
        Fila {
            caso: "timer-repeat",
            programa: "var t = setTimeout(function () {}, 50);\nclearTimeout(t);\n",
            back: Back::Rompe("setTimeout(function", "setInterval(function"),
            args: &[],
            destino: Destino::Div,
            meta: &["kind: Stdout\n", "\nobserved: true\n", "\nknown: no\n", TRAZA_DIV],
        },
        // n-ruido: antes el minificado solo corria una columna; ahora `compare` no ve las
        // posiciones de los marcos (eso es `Ninguno`, igual).
        // n-int: el programa de n-ruido (el minificado cambia `function h` por `function k`)
        // va a div/ con `cabeza_marco` de n-principio: un marco con otro nombre puede ser
        // otra funcion (`stack-nombre-marco`, abajo). El positivo es un valor que sale de la
        // columna del marco (el minificado la corre): `compare` no lo puede normalizar, y con
        // el contenido fijo A' y B' dan lo mismo que B. La cuenta de lineas es para que la
        // sal (un marco de mas) cambie la salida de A.
        Fila {
            caso: "known-causa",
            programa: "var o = { valueOf() { return 1; } };\nvar s = new Error('e').stack;\nvar l = s.split('\\n');\nconsole.log(l[1].endsWith(':1:35)'), l.length);\n",
            back: Back::Rompe("s=new", "s= new"),
            args: &[],
            destino: Destino::Known,
            meta: &["\nknown: sensor, asuncion: difuzz: el contenido de Error.prototype.stack cambia al minificar (oxc no lo documenta)\n", TRAZA_CAUSA],
        },
        // Sin el `valueOf`: aca (sin `Medido`) `coercion` no se re-chequea y lo mandaria a
        // known/ por "Coercion Methods Are Pure" (MEDIDO en n-int).
        Fila {
            caso: "stack-nombre-marco",
            programa: "var f = function h() { return new Error('e').stack; };\nconsole.log(f());\n",
            back: Back::Rompe("function h(", "function k("),
            args: &[],
            destino: Destino::Div,
            meta: &["kind: Stdout\n", "\nknown: no\n", "\nclasificacion:\n  contenido: Inconcluso -> contenido_inconcluso: con la asuncion neutralizada la divergencia desaparece, pero el minificado no se comporta como el original neutralizado (B != A'): el codigo neutralizado puede tener el bug\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: ninguna (ni flag de la tabla, ni ruta, ni backlog)\n  decision: div/\n"],
        },
        // b1-b (R1-H03): con el mismo flag, un minificado que node no acepta no es asuncion.
        Fila {
            caso: "div-invalido",
            programa: "var o = { valueOf() { return 1; } };\nconsole.log(41);\n",
            back: Back::Rompe("41)", "41"),
            args: &[],
            destino: Destino::Div,
            // La nota trae tambien la del analisis (R1-H13): el minificado no parsea.
            meta: &[
                "kind: ThrowNew\n",
                "\nknown: no\n",
                "\nnota: analisis con el parser de SWC (R1-H13): minificado (lint): SWC no lo parsea, sin analizar | node --check rechaza el minificado (y acepta el original): ninguna asuncion lo explica\n",
                "\nflags_a: coercion,minificado_invalido\n",
                TRAZA_INVALIDO,
            ],
        },
        Fila {
            caso: "panic",
            programa: "console.log(41);\n",
            back: Back::Panica,
            args: &[],
            destino: Destino::Panic,
            meta: &["kind: Panic\n", "\ne2e: panic simulado del minifier"],
        },
        // R1-H05: panic con debug assertions, el oraculo minifica y no diverge.
        Fila {
            caso: "asserts",
            programa: "console.log(41);\n",
            back: Back::PanicaConOraculo,
            args: &[],
            destino: Destino::Asserts,
            meta: &["kind: SoloDebugAssertions\n", "e2e: panic simulado del minifier"],
        },
        // R1-H08: el hijo minificador no contesta en 0.3 s.
        Fila {
            caso: "recursos",
            programa: "console.log(41);\n",
            back: Back::HijoLento,
            args: &[],
            destino: Destino::Recursos,
            meta: &["kind: Recursos\ncausa: timeout\n", "subproceso: timeout: el minifier no termino en 0.3 s"],
        },
        // Escritura sobre un literal fresco que el original no tiene: lint/ (y la salida es la misma).
        Fila {
            caso: "lint",
            programa: "console.log(41);\n",
            back: Back::Rompe("console.log(41)", "[0][0]=1,console.log(41)"),
            args: &[],
            destino: Destino::Lint,
            meta: &["kind: EscriturasSobreLiterales\n", "\nen original: 0\nen minificado: 1\n"],
        },
        // Descartes: cada uno con su causa en descartes.tsv.
        Fila { caso: "parse", programa: "var = ;\n", back: Back::Identidad, args: &[], destino: Destino::Descarte("parse_otro"), meta: &[] },
        Fila {
            caso: "timeout",
            programa: "while (true) {}\n",
            back: Back::Identidad,
            args: &["--timeout-ms", "1000"],
            destino: Destino::Descarte("timeout"),
            meta: &[],
        },
        Fila { caso: "exit", programa: "process.exit(3);\n", back: Back::Identidad, args: &[], destino: Destino::Descarte("exit"), meta: &[] },
        Fila {
            caso: "enlace",
            programa: "require('./no-existe-difuzz-e2e');\n",
            back: Back::Identidad,
            args: &[],
            destino: Destino::Descarte("enlace"),
            meta: &[],
        },
        Fila {
            caso: "mudo",
            programa: "var k = 41;\n",
            back: Back::Rompe("41", "42"),
            args: &["--no-observe"],
            destino: Destino::Descarte("mudo_sin_observar"),
            meta: &[],
        },
        // R2-H06 (b6-usuario): un mudo sin nada que observar ahora tiene su fila en
        // descartes.tsv (antes no dejaba rastro y la identidad de stats no cerraba).
        Fila {
            caso: "mudo-sin-bindings",
            programa: "0;\n",
            back: Back::Rompe("0", "1"),
            args: &[],
            destino: Destino::Descarte("mudo_sin_bindings"),
            meta: &[],
        },
    ]
}

/// Donde termino `caso`: los directorios de hallazgo que tiene y su causa en
/// descartes.tsv.
fn donde(out: &Path, caso: &str) -> Vec<String> {
    let mut v: Vec<String> = ["div", "known", "panic", "asserts", "recursos", "lint"]
        .iter()
        .filter(|d| out.join(d).join(caso).exists())
        .map(|d| d.to_string())
        .collect();
    let tsv = std::fs::read_to_string(out.join("descartes.tsv")).unwrap();
    for l in tsv.lines().filter(|l| !l.starts_with('#')) {
        let mut c = l.split('\t');
        if c.next() == Some(&format!("{caso}/input.js")) {
            v.push(format!("descarte:{}", c.next().unwrap_or_default()));
        }
    }
    v
}

#[test]
fn clasificacion_de_punta_a_punta() {
    // Sin node el test no prueba nada: falla, salvo `DIFUZZ_SIN_NODE=1` (R1-H17).
    if !crate::test_util::requiere_node() {
        return;
    }
    let base: PathBuf = std::env::temp_dir().join(format!("difuzz-test-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (corpus, out) = (base.join("corpus"), base.join("out"));
    let filas = tabla();
    for f in &filas {
        std::fs::create_dir_all(corpus.join(f.caso)).unwrap();
        std::fs::write(corpus.join(f.caso).join("input.js"), f.programa).unwrap();
    }
    let comunes = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1", "--timeout-ms", "10000"];
    let cli_de = |extra: &[&str]| crate::Cli::parse(comunes.iter().chain(extra).map(|s| s.to_string()).collect()).unwrap();
    let reporter = crate::oracle::Reporter::new(&out, cli_de(&[]).cell(), crate::runner::PRELOAD);
    // La tabla de asunciones de oxc (el objetivo, D1).
    let known = crate::known::Known::load(&corpus, &out, 0, Minifier::Oxc);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    let preload = crate::runner::Preload::write_tagged("e2e").unwrap();
    let mut resumen = Vec::new();
    for f in &filas {
        let cli = cli_de(f.args);
        // M5: el runner de produccion (el sensor lo decide la politica de `--minifier`).
        let runner = crate::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
        let cfg = f.back.config(&cli);
        // Los backends falsos no se re-minifican: sin re-chequeos propios (`medido = None`).
        let celda = matches!(f.back, Back::Celda);
        let medido = celda.then(|| crate::medido_de_celda(&cli));
        let names_cfg = if celda { crate::configs_de_celda(&cli).1 } else { None };
        let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
        let mut st = crate::Stats::default();
        crate::process_case(&ctx, &runner, &cfg, names_cfg.as_ref(), &cli, &mut st, &corpus.join(f.caso).join("input.js"));
        drop(runner);
        // Las cuentas de la linea de stats, una por destino.
        let cuentas = (st.divergences, st.known, st.panics, st.solo_asserts_sin_div, st.recursos_timeout, st.lint, st.same);
        let esperadas = match f.destino {
            Destino::Div => (1, 0, 0, 0, 0, 0, 0),
            Destino::Known => (0, 1, 0, 0, 0, 0, 0),
            Destino::Panic => (0, 0, 1, 0, 0, 0, 0),
            Destino::Asserts => (0, 0, 0, 1, 0, 0, 1),
            Destino::Recursos => (0, 0, 0, 0, 1, 0, 0),
            Destino::Lint => (0, 0, 0, 0, 0, 1, 1),
            Destino::Descarte(_) => (0, 0, 0, 0, 0, 0, 0),
            Destino::Ninguno => (0, 0, 0, 0, 0, 0, 1),
        };
        assert_eq!(cuentas, esperadas, "{}: {}", f.caso, st.line());
        // Exactamente su destino, y en ningun otro.
        let esperado: Vec<String> = match f.destino {
            Destino::Descarte(c) => vec![format!("descarte:{c}")],
            Destino::Ninguno => Vec::new(),
            d => vec![d.dir().unwrap().to_string()],
        };
        assert_eq!(donde(&out, f.caso), esperado, "{}: {}", f.caso, st.line());
        if let Some(d) = f.destino.dir() {
            let meta = std::fs::read_to_string(out.join(d).join(f.caso).join("meta.txt")).unwrap();
            assert!(meta.starts_with(&format!("case: {}/input.js\n", f.caso)), "{}: {meta}", f.caso);
            for m in f.meta {
                assert!(meta.contains(m), "{}: falta {m:?} en\n{meta}", f.caso);
            }
            // La traza de la clasificacion va al final del meta de div/ y known/.
            if matches!(f.destino, Destino::Div | Destino::Known) {
                let traza = f.meta.iter().find(|m| m.starts_with("\nclasificacion:")).expect("la fila declara su traza");
                assert!(meta.ends_with(traza), "{}: la traza no es lo ultimo del meta:\n{meta}", f.caso);
            }
        }
        resumen.push(format!("{}\t{:?}\t{}", f.caso, f.destino, st.line()));
    }
    // Los 7 destinos cubiertos.
    for d in [Destino::Div, Destino::Known, Destino::Panic, Destino::Asserts, Destino::Recursos, Destino::Lint] {
        assert!(filas.iter().any(|f| f.destino == d), "{d:?} sin fila");
    }
    assert!(filas.iter().any(|f| matches!(f.destino, Destino::Descarte(_))));
    eprintln!("e2e:\n{}", resumen.join("\n"));
    drop(preload);
    let _ = std::fs::remove_dir_all(&base);
}

/// b7-8: el programa del bug de V8 con el sensor (`f1-bucle.js` de b7-8, 10-oct:
/// SIGSEGV 20/20 con el sensor) por `process_case` con el runner de oxc (con sensor
/// por politica): el caso corre entero sin el sensor (ni un `sensor_caido`, ni una
/// repeticion I3), se compara y va a div/ con el paso `sensor: omitido` en meta.txt.
/// Con `"use strict"` el patron no esta: el sensor corre (MEDIDO 0/10 SIGSEGV), ve la
/// excepcion atrapada de `extends 1` y el caso va a known/ por esa asuncion; es justo la
/// evidencia que el caso omitido no tiene (va a div/, nunca a known/ sin ella). Con swc
/// no hay sensor que omitir.
#[test]
fn sensor_omitido_de_punta_a_punta() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let base: PathBuf = std::env::temp_dir().join(format!("difuzz-test-e2e-b78-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (corpus, out) = (base.join("corpus"), base.join("out"));
    const F1: &str = "var v;\nvar basura = [];\nfor (var i = 0; i < 200; i++) {\n  basura.push(new Array(1000).fill({ i }));\n  try { class X extends 1 {} } catch {}\n}\nif (false) eval(\"\");\nconsole.log(41);\n";
    let estricto = format!("\"use strict\";\n{F1}");
    let casos = [("riesgo", F1, "oxc", 1, "div"), ("estricto", estricto.as_str(), "oxc", 0, "known"), ("swc", F1, "swc", 0, "div")];
    for (caso, js, _, _, _) in &casos {
        std::fs::create_dir_all(corpus.join(caso)).unwrap();
        std::fs::write(corpus.join(caso).join("input.js"), js).unwrap();
    }
    let comunes = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1", "--timeout-ms", "20000"];
    let cli_de = |m: &str| crate::Cli::parse(comunes.iter().chain(&["--minifier", m]).map(|s| s.to_string()).collect()).unwrap();
    let reporter = crate::oracle::Reporter::new(&out, cli_de("oxc").cell(), crate::runner::PRELOAD);
    let known = crate::known::Known::load(&corpus, &out, 0, Minifier::Oxc);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    let preload = crate::runner::Preload::write_tagged("e2e-b78").unwrap();
    for (caso, _, m, omitidos, dir) in casos {
        let cli = cli_de(m);
        let runner = crate::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
        assert_eq!(runner.con_sensor(), m == "oxc");
        let cfg = Back::Rompe("41", "42").config(&cli);
        let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let mut st = crate::Stats::default();
        crate::process_case(&ctx, &runner, &cfg, None, &cli, &mut st, &corpus.join(caso).join("input.js"));
        drop(runner);
        let div = usize::from(dir == "div");
        assert_eq!((st.sensor_omitidos, st.divergences, st.known), (omitidos, div, 1 - div), "{caso}: {}", st.line());
        assert_eq!(donde(&out, caso), [dir], "{caso}");
        let meta = std::fs::read_to_string(out.join(dir).join(caso).join("meta.txt")).unwrap();
        let paso = format!("\nclasificacion:\n  sensor: {}\n", crate::SENSOR_OMITIDO_TRAZA);
        assert_eq!(meta.contains(&paso), omitidos == 1, "{caso}: {meta}");
        if omitidos == 1 {
            // Sin repeticion I3: nada corrio con el sensor (ni murio con el).
            assert_eq!(st.sensor_repetidos, 0, "{caso}: {meta}");
        }
    }
    drop(preload);
    let _ = std::fs::remove_dir_all(&base);
}
