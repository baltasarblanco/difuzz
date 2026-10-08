//! Mejora 10: variantes deterministas de un caso del corpus.
//!
//! Dos operadores sobre el AST de SWC (solo para parsear e imprimir: no
//! depende del minifier):
//! - borrar una sentencia: top-level o de cualquier lista de sentencias (bloque,
//!   cuerpo de funcion, `case` de un switch);
//! - perturbar un literal: un numero pasa a `0`, `-0`, `NaN` o `1`; un string a
//!   `""`, `"0"` o `"dz"`. Las que no cambian nada (`0` → `0`) no se generan.
//!
//! Las variantes van solo por la pasada cruda + lint (main::process_mutants),
//! nunca por el epilogo observador: borrar una sentencia suele dejar escrituras
//! muertas, y el epilogo agregaria usos que apagan justo ese disparador. Una
//! variante muda se descarta.
//!
//! Determinista: el mismo fuente con la misma semilla da las mismas variantes.
//! El PRNG es splitmix64, propio (sin crates nuevos).

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use swc_common::{
    sync::Lrc, FileName, Globals, SourceMap, SourceMapper, Span, Spanned, DUMMY_SP, GLOBALS,
};
use swc_ecma_ast::{EsVersion, Expr, Ident, Lit, Number, Program, Stmt, Str, UnaryExpr, UnaryOp};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_parser::{parse_file_as_script, Syntax};
use swc_ecma_transforms_base::fixer::fixer;
use swc_ecma_visit::{VisitMut, VisitMutWith};

pub struct Variant {
    /// El fuente de la variante, reimpreso (sin comentarios). Es lo que se guarda
    /// tal cual junto al hallazgo.
    pub src: String,
    /// Que se cambio, para `mutacion.txt`.
    pub desc: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    /// Borrar la sentencia numero i (preorden sobre todas las listas de sentencias).
    Delete(usize),
    /// Reemplazar el literal numero i (preorden sobre `Expr::Lit` numericos y strings).
    Num(usize, NumTo),
    Str(usize, StrTo),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumTo {
    Zero,
    NegZero,
    NaN,
    One,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StrTo {
    Empty,
    Zero,
    Dz,
}

impl NumTo {
    const ALL: [NumTo; 4] = [NumTo::Zero, NumTo::NegZero, NumTo::NaN, NumTo::One];

    fn text(self) -> &'static str {
        match self {
            NumTo::Zero => "0",
            NumTo::NegZero => "-0",
            NumTo::NaN => "NaN",
            NumTo::One => "1",
        }
    }

    /// Un literal numerico nunca es negativo ni NaN: `-0` y `NaN` siempre cambian algo.
    fn changes(self, v: f64) -> bool {
        match self {
            NumTo::Zero => v != 0.0,
            NumTo::One => v != 1.0,
            NumTo::NegZero | NumTo::NaN => true,
        }
    }

    fn expr(self) -> Expr {
        let num = |value| {
            Expr::Lit(Lit::Num(Number {
                span: DUMMY_SP,
                value,
                raw: None,
            }))
        };
        match self {
            NumTo::Zero => num(0.0),
            NumTo::One => num(1.0),
            NumTo::NegZero => Expr::Unary(UnaryExpr {
                span: DUMMY_SP,
                op: UnaryOp::Minus,
                arg: Box::new(num(0.0)),
            }),
            NumTo::NaN => Expr::Ident(Ident::new_no_ctxt("NaN".into(), DUMMY_SP)),
        }
    }
}

impl StrTo {
    const ALL: [StrTo; 3] = [StrTo::Empty, StrTo::Zero, StrTo::Dz];

    fn value(self) -> &'static str {
        match self {
            StrTo::Empty => "",
            StrTo::Zero => "0",
            StrTo::Dz => "dz",
        }
    }
}

/// Lo que habia en cada sitio de literal (para no generar perturbaciones nulas).
enum Site {
    Num(f64),
    Str(String),
}

/// Recorre en preorden numerando sentencias y literales; si `op` apunta a uno,
/// lo aplica. Con `op: None` solo hace el censo. Las dos pasadas numeran igual
/// porque la sentencia a borrar se recorre antes de sacarla.
struct Mutator {
    op: Option<Op>,
    stmts: usize,
    lits: Vec<Site>,
    hit: Option<Span>,
}

impl Mutator {
    fn new(op: Option<Op>) -> Mutator {
        Mutator {
            op,
            stmts: 0,
            lits: Vec::new(),
            hit: None,
        }
    }
}

impl VisitMut for Mutator {
    fn visit_mut_stmts(&mut self, n: &mut Vec<Stmt>) {
        let mut doomed = None;
        for (i, s) in n.iter_mut().enumerate() {
            let id = self.stmts;
            self.stmts += 1;
            if self.op == Some(Op::Delete(id)) {
                doomed = Some(i);
                self.hit = Some(s.span());
            }
            s.visit_mut_with(self);
        }
        if let Some(i) = doomed {
            n.remove(i);
        }
    }

    fn visit_mut_expr(&mut self, n: &mut Expr) {
        let site = match n {
            Expr::Lit(Lit::Num(x)) => Site::Num(x.value),
            Expr::Lit(Lit::Str(x)) => Site::Str(x.value.to_string_lossy().into_owned()),
            _ => {
                n.visit_mut_children_with(self);
                return;
            }
        };
        let id = self.lits.len();
        self.lits.push(site);
        match self.op {
            Some(Op::Num(i, to)) if i == id => {
                self.hit = Some(n.span());
                *n = to.expr();
            }
            Some(Op::Str(i, to)) if i == id => {
                self.hit = Some(n.span());
                *n = Expr::Lit(Lit::Str(Str {
                    span: DUMMY_SP,
                    value: to.value().into(),
                    raw: None,
                }));
            }
            _ => {}
        }
    }
}

/// Hasta `n` variantes de `src`, distintas entre si y del original. Menos si el
/// programa no da para tantas; ninguna si no parsea como script.
pub fn variants(src: &str, n: usize, seed: u64) -> Vec<Variant> {
    if n == 0 {
        return Vec::new();
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || variants_inner(src, n, seed))
    }))
    .unwrap_or_default()
}

fn variants_inner(src: &str, n: usize, seed: u64) -> Vec<Variant> {
    let cm: Lrc<SourceMap> = Default::default();
    let Some(program) = parse(&cm, src) else {
        return Vec::new();
    };
    let mut census = Mutator::new(None);
    program.clone().visit_mut_with(&mut census);
    let Some(base) = print(&cm, program.clone()) else {
        return Vec::new();
    };

    let mut dels: Vec<Op> = (0..census.stmts).map(Op::Delete).collect();
    let mut lits: Vec<Op> = Vec::new();
    for (i, site) in census.lits.iter().enumerate() {
        match site {
            Site::Num(v) => lits.extend(NumTo::ALL.iter().filter(|t| t.changes(*v)).map(|t| Op::Num(i, *t))),
            Site::Str(s) => lits.extend(StrTo::ALL.iter().filter(|t| t.value() != s).map(|t| Op::Str(i, *t))),
        }
    }
    let mut rng = Rng(seed);
    rng.shuffle(&mut dels);
    rng.shuffle(&mut lits);

    // Alternadas (borrar, perturbar, borrar, ...): un caso con muchos literales
    // no se queda sin borrados, ni al reves.
    let mut seen = HashSet::from([base]);
    let mut out = Vec::new();
    let (mut d, mut l) = (dels.into_iter(), lits.into_iter());
    let mut turn = 0usize;
    while out.len() < n {
        turn += 1;
        let op = if turn % 2 == 1 { d.next().or_else(|| l.next()) } else { l.next().or_else(|| d.next()) };
        let Some(op) = op else {
            break;
        };
        let mut p = program.clone();
        let mut m = Mutator::new(Some(op));
        p.visit_mut_with(&mut m);
        let Some(hit) = m.hit else {
            continue;
        };
        let Some(text) = print(&cm, p) else {
            continue;
        };
        // Variante que reimprime igual que otra (o que el original): no agrega nada.
        // Variante que no reparsea: el fixer no alcanzo; se descarta.
        if seen.contains(&text) || parse(&Lrc::new(SourceMap::default()), &text).is_none() {
            continue;
        }
        seen.insert(text.clone());
        out.push(Variant {
            desc: describe(&cm, op, hit),
            src: text,
        });
    }
    out
}

fn describe(cm: &SourceMap, op: Op, hit: Span) -> String {
    let line = cm.lookup_char_pos(hit.lo).line;
    let snippet: String = cm
        .span_to_snippet(hit)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect();
    match op {
        Op::Delete(i) => format!("borrar sentencia #{i} (linea {line}): {snippet}"),
        Op::Num(i, to) => format!("literal #{i} (linea {line}): {snippet} -> {}", to.text()),
        Op::Str(i, to) => format!("literal #{i} (linea {line}): {snippet} -> {:?}", to.value()),
    }
}

fn parse(cm: &Lrc<SourceMap>, src: &str) -> Option<Program> {
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
    let mut recovered = Vec::new();
    match parse_file_as_script(&fm, Syntax::default(), EsVersion::latest(), None, &mut recovered) {
        Ok(s) if recovered.is_empty() => Some(Program::Script(s)),
        _ => None,
    }
}

/// El mismo printer que minify::emit, sin modo minify: la variante se lee a mano
/// en el triage. El fixer agrega los parentesis que pida el reemplazo (`-0` como
/// base de `**`, por ejemplo).
fn print(cm: &Lrc<SourceMap>, program: Program) -> Option<String> {
    let program = program.apply(fixer(None));
    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default(),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
        };
        emitter.emit_program(&program).ok()?;
    }
    String::from_utf8(buf).ok()
}

/// splitmix64: chico, sin estado oculto, suficiente para elegir sitios.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Fisher-Yates.
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// `fixture/foo/input.js` → `fixture/foo__mut3/input.js`. El reporter nombra el
/// hallazgo por el directorio del caso: queda `fixture__foo__mut3`.
pub fn rel_variante(rel: &Path, k: usize) -> PathBuf {
    let file = rel.file_name().map_or_else(|| "input.js".into(), |f| f.to_os_string());
    let dir = match rel.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => format!("{}__mut{k}", p.display()),
        None => format!("_root__mut{k}"),
    };
    PathBuf::from(dir).join(file)
}

/// La ruta del caso del que salio una variante (inversa de `rel_variante`), para
/// que el backlog de un caso cubra tambien a sus variantes.
pub fn rel_base(rel: &str) -> String {
    let Some(i) = rel.rfind("__mut") else {
        return rel.to_string();
    };
    let digits = rel[i + 5..].bytes().take_while(u8::is_ascii_digit).count();
    let end = i + 5 + digits;
    if digits == 0 || !(end == rel.len() || rel[end..].starts_with('/')) {
        return rel.to_string();
    }
    format!("{}{}", &rel[..i], &rel[end..])
}

#[cfg(test)]
mod tests {
    use super::{rel_base, rel_variante, variants};
    use std::path::Path;

    const SRC: &str = "var a = 5;\nvar s = \"hola\";\nfunction f(x) {\n  if (x) {\n    a = 2;\n  }\n  return a + s;\n}\nconsole.log(f(1));\n";

    fn parses(src: &str) -> bool {
        swc_common::GLOBALS.set(&Default::default(), || {
            super::parse(&Default::default(), src).is_some()
        })
    }

    #[test]
    fn misma_semilla_mismas_variantes() {
        let a: Vec<String> = variants(SRC, 6, 7).into_iter().map(|v| v.src).collect();
        let b: Vec<String> = variants(SRC, 6, 7).into_iter().map(|v| v.src).collect();
        assert_eq!(a.len(), 6);
        assert_eq!(a, b);
        let c: Vec<String> = variants(SRC, 6, 8).into_iter().map(|v| v.src).collect();
        assert_ne!(a, c);
        // Distintas entre si.
        let mut d = a.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), a.len());
        assert!(variants(SRC, 0, 7).is_empty());
        assert!(variants("var = ;", 3, 7).is_empty());
    }

    #[test]
    fn borrar_sentencias_parsea_y_alterna_con_literales() {
        // Todas: 7 sentencias (4 top-level, if, a = 2, return) y 4 literales.
        let all = variants(SRC, 100, 1);
        let dels: Vec<_> = all.iter().filter(|v| v.desc.starts_with("borrar")).collect();
        assert_eq!(dels.len(), 7);
        for v in &all {
            assert!(parses(&v.src), "no parsea: {}\n{}", v.desc, v.src);
        }
        // Borrar el `if` se lleva su bloque; borrar `a = 2` deja el bloque vacio.
        assert!(all.iter().any(|v| !v.src.contains("if") && v.src.contains("return a + s")));
        assert!(all.iter().any(|v| v.src.contains("if (x) {}")));
        // Las primeras alternan borrar / perturbar.
        let first = variants(SRC, 4, 1);
        let kinds: Vec<bool> = first.iter().map(|v| v.desc.starts_with("borrar")).collect();
        assert_eq!(kinds, [true, false, true, false]);
    }

    #[test]
    fn perturbaciones_esperadas() {
        let all = variants("var a = 5, z = 0, s = \"\";\n", 100, 3);
        let mut descs: Vec<&str> = all.iter().map(|v| v.desc.as_str()).collect();
        descs.sort();
        // 5: cuatro cambios; 0: sin `0`; "": sin `""`; y la unica sentencia.
        assert_eq!(all.len(), 1 + 4 + 3 + 2, "{descs:#?}");
        for want in ["var a = 0,", "var a = -0,", "var a = NaN,", "var a = 1,", "z = -0,", "z = NaN,", "z = 1,", "s = \"0\"", "s = \"dz\""] {
            assert!(all.iter().any(|v| v.src.contains(want)), "falta {want}");
        }
        assert!(!descs.iter().any(|d| d.starts_with("literal #1 ") && d.ends_with("-> 0")), "{descs:#?}");
        assert!(!descs.iter().any(|d| d.starts_with("literal #2 ") && d.ends_with("-> \"\"")), "{descs:#?}");
        // -0 como base de ** necesita parentesis.
        let pow = variants("var p = 2 ** 3;\n", 100, 3);
        assert!(pow.iter().any(|v| v.src.contains("(-0) ** 3")), "{:#?}", pow.iter().map(|v| &v.src).collect::<Vec<_>>());
    }

    #[test]
    fn rutas_de_variantes() {
        let r = rel_variante(Path::new("fixture/issues/9739/input.js"), 3);
        assert_eq!(r, Path::new("fixture/issues/9739__mut3/input.js"));
        assert_eq!(rel_base(&r.to_string_lossy()), "fixture/issues/9739/input.js");
        assert_eq!(rel_variante(Path::new("input.js"), 1), Path::new("_root__mut1/input.js"));
        assert_eq!(rel_base("a/b__mutx/input.js"), "a/b__mutx/input.js");
        assert_eq!(rel_base("a/b/input.js"), "a/b/input.js");
    }
}
