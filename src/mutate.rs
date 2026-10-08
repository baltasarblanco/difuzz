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
use swc_ecma_ast::{
    Decl, Expr, Ident, Lit, ModuleItem, Number, Pat, Program, Stmt, Str, UnaryExpr, UnaryOp, VarDeclKind,
};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};

use crate::minify::SourceKind;
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

/// M18 (e): `"use strict"` y otras directivas del prologo (strings sueltos al
/// principio de una lista de sentencias) no se borran ni se perturban: cambian
/// el modo del programa, no prueban el minifier.
fn is_directive(s: &Stmt) -> bool {
    matches!(s, Stmt::Expr(e) if matches!(&*e.expr, Expr::Lit(Lit::Str(_))))
}

/// M18 (b): borrar una declaracion `var x = e` deja `x` sin declarar (global
/// implicito, ruido sloppy). En su lugar se borra el inicializador: `var x`.
/// `const` pasa a `let` (un const sin inicializador no parsea). Con patrones
/// (`var {a} = o`) no se puede: `false`, y el llamador borra la sentencia.
fn strip_init(s: &mut Stmt) -> bool {
    let Stmt::Decl(Decl::Var(v)) = s else {
        return false;
    };
    if !v.decls.iter().any(|d| d.init.is_some()) || !v.decls.iter().all(|d| matches!(d.name, Pat::Ident(_))) {
        return false;
    }
    if v.kind == VarDeclKind::Const {
        v.kind = VarDeclKind::Let;
    }
    for d in &mut v.decls {
        d.init = None;
    }
    true
}

impl VisitMut for Mutator {
    fn visit_mut_stmts(&mut self, n: &mut Vec<Stmt>) {
        let mut doomed = None;
        let prologue = n.iter().take_while(|s| is_directive(s)).count();
        for (i, s) in n.iter_mut().enumerate().skip(prologue) {
            let id = self.stmts;
            self.stmts += 1;
            if self.op == Some(Op::Delete(id)) {
                doomed = Some(i);
                self.hit = Some(s.span());
            }
            s.visit_mut_with(self);
        }
        if let Some(i) = doomed {
            if !strip_init(&mut n[i]) {
                n.remove(i);
            }
        }
    }

    /// M15: la lista top-level de un modulo. Solo se borran sentencias; los
    /// import/export no (borrarlos rompe el enlace o la forma del modulo), pero
    /// se recorren (un `export function` tiene sentencias adentro).
    fn visit_mut_module_items(&mut self, n: &mut Vec<ModuleItem>) {
        let mut doomed = None;
        let prologue = n.iter().take_while(|it| matches!(it, ModuleItem::Stmt(s) if is_directive(s))).count();
        for (i, item) in n.iter_mut().enumerate().skip(prologue) {
            if let ModuleItem::Stmt(s) = item {
                let id = self.stmts;
                self.stmts += 1;
                if self.op == Some(Op::Delete(id)) {
                    doomed = Some(i);
                    self.hit = Some(s.span());
                }
            }
            item.visit_mut_with(self);
        }
        if let Some(i) = doomed {
            let stripped = match &mut n[i] {
                ModuleItem::Stmt(s) => strip_init(s),
                _ => false,
            };
            if !stripped {
                n.remove(i);
            }
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

/// Carril A (script): las de siempre. Solo para los tests.
#[cfg(test)]
pub fn variants(src: &str, n: usize, seed: u64) -> Vec<Variant> {
    variants_for(src, n, seed, SourceKind::Script).unwrap_or_default()
}

/// Hasta `n` variantes de `src`, distintas entre si y del original. Menos si el
/// programa no da para tantas. M15: se parsea, reparsea e imprime segun el
/// carril (`kind`); `None` si el base no parsea (MutStats.no_parsea), para que
/// no quede como "0 variantes" en silencio.
pub fn variants_for(src: &str, n: usize, seed: u64, kind: SourceKind) -> Option<Vec<Variant>> {
    if n == 0 {
        return Some(Vec::new());
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || variants_inner(src, n, seed, kind))
    }))
    .unwrap_or(Some(Vec::new()))
}

fn variants_inner(src: &str, n: usize, seed: u64, kind: SourceKind) -> Option<Vec<Variant>> {
    let cm: Lrc<SourceMap> = Default::default();
    let program = parse(&cm, src, kind)?;
    let mut census = Mutator::new(None);
    program.clone().visit_mut_with(&mut census);
    let Some(base) = print(&cm, program.clone()) else {
        return Some(Vec::new());
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
        if seen.contains(&text) || parse(&Lrc::new(SourceMap::default()), &text, kind).is_none() {
            continue;
        }
        seen.insert(text.clone());
        out.push(Variant {
            desc: describe(&cm, op, hit),
            src: text,
        });
    }
    Some(out)
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

/// Nota transversal 2: el mismo parse por carril que backend/swc.rs (con este
/// SourceMap, que `describe` usa para el fragmento).
fn parse(cm: &Lrc<SourceMap>, src: &str, kind: SourceKind) -> Option<Program> {
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
    crate::backend::swc::parse_program(&fm, kind)
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
            super::parse(&Default::default(), src, crate::minify::SourceKind::Script).is_some()
        })
    }

    /// M15: un modulo (import/export/await top-level) genera variantes en el
    /// carril B, y no parsea en el A (None, no "0 variantes").
    #[test]
    fn variantes_de_modulos() {
        use super::variants_for;
        use crate::minify::SourceKind;
        let esm = "import { x } from './y.mjs';\nexport const a = 1;\nconst b = await x;\nconsole.log(a + 2, \"s\", b);\n";
        let vs = variants_for(esm, 4, 1, SourceKind::Module).expect("parsea como modulo");
        assert!(!vs.is_empty());
        for v in &vs {
            assert!(v.src.contains("import") && v.src.contains("export"), "{}", v.src);
        }
        assert!(variants_for(esm, 4, 1, SourceKind::Script).is_none());
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

    /// M18 (b) y (e): borrar una declaracion con inicializador borra el
    /// inicializador; las directivas no se tocan.
    #[test]
    fn borrar_declaracion_y_directivas() {
        let all = variants("\"use strict\";\nconst k = 5;\nvar {q} = o;\nfunction f() {\n  \"use strict\";\n  return k;\n}\n", 100, 1);
        let src: Vec<&str> = all.iter().map(|v| v.src.as_str()).collect();
        assert!(src.iter().any(|s| s.contains("let k;")), "{src:#?}");
        // El patron no tiene version sin inicializador: se borra entera.
        assert!(src.iter().any(|s| !s.contains("q") && s.contains("const k = 5")), "{src:#?}");
        // (Borrar `function f` se lleva su directiva; la de arriba queda siempre.)
        for s in &src {
            assert!(s.starts_with("\"use strict\";"), "{s}");
        }
        assert!(!all.iter().any(|v| (v.desc.starts_with("literal") && v.desc.contains("use strict")) || v.desc.contains("): \"use strict\"")), "{:#?}", all.iter().map(|v| &v.desc).collect::<Vec<_>>());
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
