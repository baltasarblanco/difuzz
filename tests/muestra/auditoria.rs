//! 9e: auditorias A1, A2 y C1 de la revision-1 (07-oct) sobre la muestra fija
//! `tests/muestra/casos/` (100 fixtures de SWC; criterio y origen en
//! `tests/muestra/README.md`, lista en `LISTA.tsv`).
//!
//! difuzz es solo binario (no hay `difuzz::` que un test de `tests/` pueda
//! importar): este archivo se incluye con `#[path]` desde `src/backend/oxc.rs`.
//! Va ahi y no en main.rs porque A1 arma un `Oxc` sin compresion ni mangle, y
//! los campos de `Oxc` son privados del modulo `backend::oxc` (un modulo hijo
//! los ve; uno declarado en main.rs no).
//!
//! - A1 (mismo printer): con compresion y mangle apagados, el minificado es
//!   byte a byte el original. Si no, el printer o el camino de impresion del
//!   minificado difiere del original y difuzz confundiria eso con un bug.
//! - A2 (original estable): imprimir el original re-parseado da el mismo
//!   texto (preset core_defaults, el que corre difuzz).
//! - C1 (lo que usa la gente): el minificado de oxc en difuzz es byte a byte el
//!   de `minifySync` de `oxc-minify` (npm, `probe/`) con las mismas opciones.
//!   Necesita node y `probe/node_modules`: `#[ignore]`.
//!
//! Las excepciones van listadas con su causa. Una excepcion que deja de
//! ocurrir tambien falla: la lista no puede quedar vieja en silencio.
//!
//! Correr: `cargo test muestra::` (A1, A2) y
//! `cargo test muestra::c1 -- --ignored --nocapture` (C1).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::Oxc;
use crate::backend::swc::Swc;
use crate::backend::{Backend, Minifier};
use crate::minify::{self, Config, Outcome, Preset, SourceKind};

const KINDS: [SourceKind; 2] = [SourceKind::Script, SourceKind::Module];
const BACKENDS: [Minifier; 2] = [Minifier::Swc, Minifier::Oxc];

/// Un caso que no cumple la auditoria, con la causa medida.
struct Excepcion {
    backend: Minifier,
    carril: SourceKind,
    caso: &'static str,
    causa: &'static str,
}

/// A1. Medidas por la revision-1 sobre el corpus completo (INFORME.md, fila A1:
/// swc B 3051/3053) y de nuevo aca sobre la muestra.
const EXCEPCIONES_A1: &[Excepcion] = &[
    Excepcion {
        backend: Minifier::Swc,
        carril: SourceKind::Module,
        caso: "terser/compress/export/issue_2126",
        causa: "swc: `optimize` fusiona y reordena `export {...}` aun con compress y mangle en None (R1-H21)",
    },
    Excepcion {
        backend: Minifier::Swc,
        carril: SourceKind::Module,
        caso: "terser/compress/export/redirection",
        causa: "swc: `optimize` fusiona y reordena `export {...}` aun con compress y mangle en None (R1-H21)",
    },
];

/// A2: sin excepciones conocidas (revision-1: 100 % en los dos backends).
const EXCEPCIONES_A2: &[Excepcion] = &[];

/// C1 (solo oxc): sin excepciones conocidas. El campo `backend` es siempre oxc;
/// el preset va en la causa si hiciera falta distinguir.
const EXCEPCIONES_C1: &[(Preset, Excepcion)] = &[];

/// Minimo de casos comparados por celda (backend x carril): una muestra que
/// casi no parsea no puede pasar. Medido 09-oct: el minimo real es 73
/// (carril A: 27 casos no parsean como script). Revisar al cambiar la muestra.
const MIN_COMPARADOS: usize = 65;

fn dir_muestra() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/muestra")
}

/// Los casos de la muestra: (ruta relativa a `casos/`, fuente). Falla si no
/// son exactamente los de `LISTA.tsv`: una muestra incompleta no puede pasar.
fn muestra() -> Vec<(String, String)> {
    let casos = dir_muestra().join("casos");
    let lista = std::fs::read_to_string(dir_muestra().join("LISTA.tsv")).expect("falta tests/muestra/LISTA.tsv");
    let mut esperados: Vec<&str> = lista
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("ruta\t"))
        .map(|l| l.split('\t').next().unwrap_or_default())
        .collect();
    esperados.sort_unstable();
    let mut hallados: Vec<(String, String)> = crate::corpus::collect(&casos)
        .into_iter()
        .map(|p| {
            let rel = p.parent().and_then(|d| d.strip_prefix(&casos).ok()).expect("ruta de caso").to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (rel, src)
        })
        .collect();
    hallados.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let nombres: Vec<&str> = hallados.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        nombres, esperados,
        "tests/muestra/casos no coincide con LISTA.tsv: verificar con `node tests/muestra/regenerar.cjs`"
    );
    assert!(!hallados.is_empty(), "muestra vacia");
    hallados
}

/// Corre `f` en un hilo con la pila de los workers (M1): los fixtures anidados
/// desbordan la pila de 2 MiB de un hilo de test.
fn con_pila<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|s| {
        let h = std::thread::Builder::new().stack_size(crate::WORKER_STACK).spawn_scoped(s, f).expect("hilo con pila grande");
        h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

fn primera_diferencia(a: &str, b: &str) -> String {
    let i = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let ctx = |s: &str| {
        let lo = s.floor_char_boundary(i.saturating_sub(40));
        let hi = s.ceil_char_boundary((i + 40).min(s.len()));
        s[lo..hi].replace('\n', "\\n")
    };
    format!("byte {i}: {:?} | {:?}", ctx(a), ctx(b))
}

fn rotulo(o: &Outcome) -> &'static str {
    match o {
        Outcome::Ok { .. } => "ok",
        Outcome::ParseError { .. } => "no parsea",
        Outcome::CodegenError => "codegen",
        Outcome::Panic(_) => "panic",
    }
}

/// A1: el backend de produccion con compresion y mangle apagados. Lo demas
/// (parser, printer, pasos fijos del backend) es lo mismo que corre difuzz.
fn sin_optimizar(m: Minifier, kind: SourceKind) -> Box<dyn Backend> {
    match m {
        Minifier::Swc => Box::new(Swc {
            minify: swc_ecma_minifier::option::MinifyOptions { compress: None, mangle: None, ..Default::default() },
            kind,
        }),
        Minifier::Oxc => Box::new(Oxc {
            options: oxc_minifier::MinifierOptions { compress: None, mangle: None, mangle_properties: None },
            source_type: crate::backend::source_type(kind),
        }),
        Minifier::Identidad => unreachable!("la muestra no usa el backend de prueba"),
    }
}

#[derive(Default)]
struct Celda {
    comparados: usize,
    cumplen: usize,
    excepciones: usize,
    no_parsea: usize,
    otros: usize,
}

/// Resume una auditoria: tabla por celda en stdout y panic con todas las
/// fallas juntas (no la primera).
fn cerrar(nombre: &str, celdas: &[(Minifier, SourceKind, Celda)], fallas: &[String]) {
    let mut t = format!("{nombre}: backend carril comparados cumplen excepciones no_parsea otros\n");
    for (m, k, c) in celdas {
        let _ = writeln!(t, "{nombre}: {} {} {} {} {} {} {}", m.name(), k.carril(), c.comparados, c.cumplen, c.excepciones, c.no_parsea, c.otros);
    }
    print!("{t}");
    let pocos: Vec<String> = celdas
        .iter()
        .filter(|(_, _, c)| c.comparados < MIN_COMPARADOS)
        .map(|(m, k, c)| format!("{}/{}: solo {} comparados (minimo {MIN_COMPARADOS})", m.name(), k.carril(), c.comparados))
        .collect();
    let todas: Vec<&String> = fallas.iter().chain(&pocos).collect();
    assert!(todas.is_empty(), "{nombre}: {} fallas\n{}\n{t}", todas.len(), todas.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
}

/// Una excepcion listada que ya no ocurre, o que no se pudo comprobar.
fn excepciones_sin_usar(nombre: &str, lista: &[Excepcion], usadas: &[(Minifier, SourceKind, &str)]) -> Vec<String> {
    lista
        .iter()
        .filter(|e| !usadas.iter().any(|(m, k, c)| *m == e.backend && *k == e.carril && *c == e.caso))
        .map(|e| format!("{nombre}: la excepcion {}/{} {} ya no ocurre o no se comprobo ({}): sacarla de la lista", e.backend.name(), e.carril.carril(), e.caso, e.causa))
        .collect()
}

#[test]
fn a1_mismo_printer() {
    let casos = muestra();
    let (celdas, fallas) = con_pila(|| {
        let mut celdas = Vec::new();
        let mut fallas = Vec::new();
        let mut usadas = Vec::new();
        for m in BACKENDS {
            for kind in KINDS {
                let mut c = Celda::default();
                for (rel, src) in &casos {
                    let exc = EXCEPCIONES_A1.iter().find(|e| e.backend == m && e.carril == kind && e.caso == rel);
                    match minify::prepare(src, &Config::from_backend(sin_optimizar(m, kind))) {
                        Outcome::Ok { original, minified, .. } => {
                            c.comparados += 1;
                            match (original == minified, exc) {
                                (true, None) => c.cumplen += 1,
                                (false, Some(e)) => {
                                    c.excepciones += 1;
                                    usadas.push((m, kind, e.caso));
                                }
                                (false, None) => fallas.push(format!(
                                    "A1 {}/{} {rel}: sin compresion ni mangle el minificado difiere del original, {}",
                                    m.name(),
                                    kind.carril(),
                                    primera_diferencia(&original, &minified)
                                )),
                                (true, Some(_)) => {} // queda sin usar: falla abajo
                            }
                        }
                        Outcome::ParseError { .. } => c.no_parsea += 1,
                        o => {
                            c.otros += 1;
                            fallas.push(format!("A1 {}/{} {rel}: {} con todo apagado", m.name(), kind.carril(), rotulo(&o)));
                        }
                    }
                }
                celdas.push((m, kind, c));
            }
        }
        fallas.extend(excepciones_sin_usar("A1", EXCEPCIONES_A1, &usadas));
        (celdas, fallas)
    });
    cerrar("A1", &celdas, &fallas);
}

#[test]
fn a2_original_estable() {
    let casos = muestra();
    let (celdas, fallas) = con_pila(|| {
        let mut celdas = Vec::new();
        let mut fallas = Vec::new();
        let mut usadas = Vec::new();
        for m in BACKENDS {
            for kind in KINDS {
                let cfg = Config::from_backend(m.build(Preset::CoreDefaults, kind));
                let mut c = Celda::default();
                for (rel, src) in &casos {
                    let exc = EXCEPCIONES_A2.iter().find(|e| e.backend == m && e.carril == kind && e.caso == rel);
                    let o1 = match minify::prepare(src, &cfg) {
                        Outcome::Ok { original, .. } => original,
                        Outcome::ParseError { .. } => {
                            c.no_parsea += 1;
                            continue;
                        }
                        // Un panic del minifier no deja original: no entra en A2
                        // (revision-1 igual). El minimo de comparados lo acota.
                        _ => {
                            c.otros += 1;
                            continue;
                        }
                    };
                    c.comparados += 1;
                    let estable = match minify::prepare(&o1, &cfg) {
                        Outcome::Ok { original: o2, .. } if o2 == o1 => Ok(()),
                        Outcome::Ok { original: o2, .. } => Err(primera_diferencia(&o1, &o2)),
                        o => Err(format!("el original re-impreso da {}", rotulo(&o))),
                    };
                    match (estable, exc) {
                        (Ok(()), None) => c.cumplen += 1,
                        (Err(_), Some(e)) => {
                            c.excepciones += 1;
                            usadas.push((m, kind, e.caso));
                        }
                        (Err(d), None) => fallas.push(format!("A2 {}/{} {rel}: el original no es estable, {d}", m.name(), kind.carril())),
                        (Ok(()), Some(_)) => {}
                    }
                }
                celdas.push((m, kind, c));
            }
        }
        fallas.extend(excepciones_sin_usar("A2", EXCEPCIONES_A2, &usadas));
        (celdas, fallas)
    });
    cerrar("A2", &celdas, &fallas);
}

/// C1: corre `c1-npm.cjs` sobre los minificados ya escritos en `trabajo` y
/// devuelve su stdout. Sin hijos propios: matar a node alcanza.
fn correr_node(nm: &Path, trabajo: &Path) -> String {
    let out = std::fs::File::create(trabajo.join("node.out")).unwrap();
    let err = std::fs::File::create(trabajo.join("node.err")).unwrap();
    let mut hijo = std::process::Command::new("node")
        .arg(dir_muestra().join("c1-npm.cjs"))
        .arg(nm)
        .arg(trabajo)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .expect("lanzar node");
    let limite = Instant::now() + Duration::from_secs(120);
    let estado = loop {
        if let Some(s) = hijo.try_wait().expect("esperar a node") {
            break s;
        }
        if Instant::now() > limite {
            let _ = hijo.kill();
            let _ = hijo.wait();
            panic!("C1: node no termino en 120 s (trabajo en {})", trabajo.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stderr = std::fs::read_to_string(trabajo.join("node.err")).unwrap_or_default();
    assert!(estado.success(), "C1: c1-npm.cjs fallo ({estado}):\n{stderr}");
    std::fs::read_to_string(trabajo.join("node.out")).unwrap()
}

#[test]
#[ignore = "necesita node y probe/node_modules/oxc-minify (npm ci en probe/)"]
fn c1_igual_a_oxc_minify_de_npm() {
    // R1-H17: sin node falla, salvo `DIFUZZ_SIN_NODE=1` (test_util.rs).
    if !crate::test_util::requiere_node() {
        return;
    }
    let nm = Path::new(env!("CARGO_MANIFEST_DIR")).join("probe/node_modules");
    let pkg = std::fs::read_to_string(nm.join("oxc-minify/package.json"))
        .unwrap_or_else(|e| panic!("C1: falta {}/oxc-minify ({e}): correr `npm ci` en probe/", nm.display()));
    // La version de npm tiene que ser la del crate (version() == Cargo.lock lo
    // cuida version_tests.rs).
    let crate_ver = Minifier::Oxc.version().rsplit(' ').next().unwrap();
    assert!(
        pkg.contains(&format!("\"version\": \"{crate_ver}\"")),
        "C1: probe/node_modules/oxc-minify no es {crate_ver} (la del crate): fijar oxc-minify a esa version en probe/package.json"
    );

    let casos = muestra();
    let trabajo = std::env::temp_dir().join(format!("difuzz-test-muestra-c1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&trabajo);
    std::fs::create_dir_all(&trabajo).unwrap();
    let casos_dir = dir_muestra().join("casos");

    // Lado difuzz: el minificado de cada preset x carril, como en una celda.
    let (pedidos, mut celdas, mut fallas) = con_pila(|| {
        let mut pedidos: Vec<(String, Preset, SourceKind, String)> = Vec::new();
        let mut celdas = Vec::new();
        let mut fallas = Vec::new();
        for preset in [Preset::CoreDefaults, Preset::CarrilA] {
            for kind in KINDS {
                let cfg = Config::from_backend(Minifier::Oxc.build(preset, kind));
                let mut c = Celda::default();
                for (i, (rel, src)) in casos.iter().enumerate() {
                    match minify::prepare(src, &cfg) {
                        Outcome::Ok { minified, .. } => {
                            let id = format!("{}-{}-{i}", preset.name(), kind.carril());
                            std::fs::write(trabajo.join(format!("{id}.difuzz.js")), minified).unwrap();
                            pedidos.push((id, preset, kind, rel.clone()));
                        }
                        Outcome::ParseError { .. } => c.no_parsea += 1,
                        o => {
                            c.otros += 1;
                            fallas.push(format!("C1 {}/{} {rel}: difuzz da {}", preset.name(), kind.carril(), rotulo(&o)));
                        }
                    }
                }
                celdas.push(((preset, kind), c));
            }
        }
        (pedidos, celdas, fallas)
    });
    let tsv: String = pedidos
        .iter()
        .map(|(id, p, k, rel)| format!("{id}\t{}\t{}\t{}\n", k.carril(), p.name(), casos_dir.join(rel).join("input.js").display()))
        .collect();
    std::fs::write(trabajo.join("pedidos.tsv"), tsv).unwrap();

    // Lado npm.
    let salida = correr_node(&nm, &trabajo);
    let mut usadas: Vec<(Preset, SourceKind, &str)> = Vec::new();
    let mut vistos = 0;
    for linea in salida.lines().filter(|l| !l.is_empty()) {
        let mut campos = linea.splitn(3, '\t');
        let (id, res, det) = (campos.next().unwrap(), campos.next().unwrap_or(""), campos.next().unwrap_or(""));
        let Some((_, preset, kind, rel)) = pedidos.iter().find(|p| p.0 == id) else {
            fallas.push(format!("C1: node devolvio un id desconocido: {linea}"));
            continue;
        };
        vistos += 1;
        let c = &mut celdas.iter_mut().find(|(pk, _)| *pk == (*preset, *kind)).unwrap().1;
        c.comparados += 1;
        let exc = EXCEPCIONES_C1.iter().find(|(p, e)| p == preset && e.carril == *kind && e.caso == rel);
        match (res, exc) {
            ("igual", None) => c.cumplen += 1,
            ("igual", Some(_)) => {}
            (_, Some((p, e))) => {
                c.excepciones += 1;
                usadas.push((*p, e.carril, e.caso));
            }
            _ => fallas.push(format!("C1 {}/{} {rel}: {res} {det}", preset.name(), kind.carril())),
        }
    }
    if vistos != pedidos.len() {
        fallas.push(format!("C1: node respondio {vistos} de {} pedidos", pedidos.len()));
    }
    for (p, e) in EXCEPCIONES_C1 {
        if !usadas.iter().any(|(up, uk, uc)| up == p && *uk == e.carril && *uc == e.caso) {
            fallas.push(format!("C1: la excepcion {}/{} {} ya no ocurre ({}): sacarla", p.name(), e.carril.carril(), e.caso, e.causa));
        }
    }

    let mut t = String::from("C1: preset carril comparados iguales excepciones no_parsea otros\n");
    for ((p, k), c) in &celdas {
        let _ = writeln!(t, "C1: {} {} {} {} {} {} {}", p.name(), k.carril(), c.comparados, c.cumplen, c.excepciones, c.no_parsea, c.otros);
        if c.comparados < MIN_COMPARADOS {
            fallas.push(format!("C1 {}/{}: solo {} comparados (minimo {MIN_COMPARADOS})", p.name(), k.carril(), c.comparados));
        }
    }
    print!("{t}");
    assert!(fallas.is_empty(), "C1: {} fallas (trabajo en {})\n{}\n{t}", fallas.len(), trabajo.display(), fallas.join("\n"));
    std::fs::remove_dir_all(&trabajo).unwrap();
}
