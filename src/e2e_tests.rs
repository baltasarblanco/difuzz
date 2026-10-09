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
                let (static_flags, dead_writes, analisis) = crate::backend::swc::checks_on_text(src, &original, &minified, SourceKind::Script);
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
}

impl Back {
    fn config(&self) -> Config {
        let id = || Box::new(Identidad { kind: SourceKind::Script });
        match *self {
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

/// b4-a: al pausar en la excepcion atrapada, V8 formatea `.stack` (riesgo
/// documentado del sensor): B gana `stack` y el re-chequeo de contenido lo
/// descarta (`contenido_irrelevante` en A).
const TRAZA_TDZ_ATRAPADA: &str = "\nclasificacion:\n  contenido: NoExplica -> contenido_irrelevante (A con toString/.stack cambiados da lo mismo)\n  profundidad: no aplica (A con mas pila da lo mismo, o la corrida honda fallo)\n  asunciones: sensor, asuncion: oxc: No TDZ Violation (atrapada)\n  minificado_invalido: no (node --check acepta el minificado)\n  sintoma: el de known-sintomas.tsv (o primero registrado)\n  decision: known/ (sensor, asuncion: oxc: No TDZ Violation (atrapada))\n";

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
        // inspector) -> asuncion de la tabla de oxc. Con `medido = None` no corre el
        // re-chequeo de violaciones (lo cubre `clasificar::tdz_atrapada_de_punta_a_punta`).
        Fila {
            caso: "known-violacion",
            programa: "try { x; } catch (e) {}\nlet x = 1;\nconsole.log(41);\n",
            back: Back::Rompe("41", "42"),
            args: &[],
            destino: Destino::Known,
            meta: &["\nknown: sensor, asuncion: oxc: No TDZ Violation (atrapada)\n", "\nflags_a: tdz_atrapada,contenido_irrelevante\nflags_b: stack,tdz_atrapada\n", TRAZA_TDZ_ATRAPADA],
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
    let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
    let preload = crate::runner::Preload::write_tagged("e2e").unwrap();
    let mut resumen = Vec::new();
    for f in &filas {
        let cli = cli_de(f.args);
        let runner = crate::runner::Runner::new(&preload, crate::test_util::id_runner(), cli.timeout);
        let cfg = f.back.config();
        let mut st = crate::Stats::default();
        crate::process_case(&ctx, &runner, &cfg, None, &cli, &mut st, &corpus.join(f.caso).join("input.js"));
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
