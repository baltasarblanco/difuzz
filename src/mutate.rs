//! Mejora 10: variantes deterministas de un caso del corpus.
//!
//! Dos operadores sobre el AST de SWC (solo para parsear e imprimir: no
//! depende del minifier):
//! - borrar una sentencia: top-level o de cualquier lista de sentencias (bloque,
//!   cuerpo de funcion, `case` de un switch);
//! - perturbar un literal: un numero pasa a `0`, `-0`, `NaN` o `1`; un string a
//!   `""`, `"0"` o `"dz"`. Las que no cambian nada (`0` → `0`) no se generan.
//!
//! M33 (`--mutaciones-extra N`, opt-in): operadores nuevos, en el orden del
//! consolidado: literal a negativo (`n` → `-n`, `-1`), envolver una sentencia
//! en un bloque o en una IIFE flecha, duplicarla, negar la condicion de un `if`
//! o intercambiar sus ramas, `var` <-> `let`/`const` (filtrado por `tdz` en
//! main) y `"use strict"` (programa o funcion, solo carril A). Se generan
//! DESPUES de las clasicas, con su propio PRNG y numeradas desde
//! `--mutaciones` + 1: una `__mutK` clasica es la misma con o sin extras, y sin
//! el flag nada cambia (los hallazgos ya guardados siguen valiendo).
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
    ArrowExpr, ArrowFunctionBody, BlockStmt, CallExpr, Callee, Class, Decl, Expr, ExprStmt, Function, FunctionBody, Ident, Lit,
    ModuleItem, Number, ParenExpr, Pat, Program, Stmt, Str, UnaryExpr, UnaryOp, VarDecl, VarDeclKind,
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
    /// M33: salio de los operadores nuevos (`--mutaciones-extra`): se numera
    /// despues de las clasicas.
    pub extra: bool,
    /// M33: `var` <-> `let`/`const`. Si el original de la variante prende el flag
    /// `tdz`, la variante se descarta (es ruido del operador, no del minifier).
    pub tdz_filter: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    /// Borrar la sentencia numero i (preorden sobre todas las listas de sentencias).
    Delete(usize),
    /// Reemplazar el literal numero i (preorden sobre `Expr::Lit` numericos y strings).
    Num(usize, NumTo),
    Str(usize, StrTo),
    /// M33: operadores nuevos (solo con `--mutaciones-extra`).
    X(Extra),
}

/// M33: operadores nuevos. Numeran con los mismos censos que los clasicos
/// (sentencias, literales) mas dos propios (declaraciones y funciones).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Extra {
    /// Literal numerico i: `n` → `-n` o `-1` (de ahi salio O3, la clave computada negativa).
    Neg(usize, NegTo),
    /// Envolver la sentencia i en un bloque o en una IIFE flecha (conserva `this` y `arguments`).
    Wrap(usize, WrapTo),
    /// Duplicar la sentencia i.
    Dup(usize),
    /// `if` (sentencia i): negar la condicion o intercambiar las ramas.
    If(usize, IfTo),
    /// La declaracion i (`var`/`let`/`const`, preorden) pasa a otro tipo.
    Var(usize, VarDeclKind),
    /// `"use strict"` al principio del programa (`None`) o de la funcion i. Solo carril A.
    Strict(Option<usize>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NegTo {
    Opp,
    MinusOne,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WrapTo {
    Block,
    Iife,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IfTo {
    Negate,
    Swap,
}

impl NegTo {
    fn expr(self, v: f64) -> Expr {
        let arg = match self {
            NegTo::Opp => v,
            NegTo::MinusOne => 1.0,
        };
        Expr::Unary(UnaryExpr {
            span: DUMMY_SP,
            op: UnaryOp::Minus,
            arg: Box::new(Expr::Lit(Lit::Num(Number { span: DUMMY_SP, value: arg, raw: None }))),
        })
    }
}

/// M33: que operadores admite una sentencia (se decide en el censo).
#[derive(Clone, Copy, Default)]
struct StmtSite {
    /// Envolver en `{}`: no una declaracion lexica (dejaria de verse afuera) ni
    /// una `function` (en un bloque es Annex B, ruido sloppy).
    block: bool,
    /// Envolver en `(() => {...})()`: tampoco `var` ni saltos (`return`, `break`).
    iife: bool,
    dup: bool,
    /// `Some(tiene_else)` si es un `if`.
    if_alt: Option<bool>,
}

fn stmt_site(s: &Stmt) -> StmtSite {
    let var = matches!(s, Stmt::Decl(Decl::Var(v)) if v.kind == VarDeclKind::Var);
    let decl = matches!(s, Stmt::Decl(_));
    let trivial = matches!(s, Stmt::Block(_) | Stmt::Empty(_));
    let jump = matches!(s, Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break(_) | Stmt::Continue(_));
    StmtSite {
        block: (var || !decl) && !trivial,
        iife: !decl && !trivial && !jump,
        dup: (var || !decl) && !trivial && !jump,
        if_alt: match s {
            Stmt::If(i) => Some(i.alt.is_some()),
            _ => None,
        },
    }
}

/// M33: una declaracion `var`/`let`/`const`.
#[derive(Clone, Copy)]
struct VarSite {
    kind: VarDeclKind,
    all_init: bool,
}

fn is_use_strict(s: &Stmt) -> bool {
    matches!(s, Stmt::Expr(e) if matches!(&*e.expr, Expr::Lit(Lit::Str(x)) if x.value.to_string_lossy() == "use strict"))
}

fn use_strict() -> Stmt {
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Lit(Lit::Str(Str { span: DUMMY_SP, value: "use strict".into(), raw: None }))),
    })
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
    /// M33: censos de los operadores nuevos (no cambian la numeracion de los clasicos).
    sites: Vec<StmtSite>,
    vars: Vec<VarSite>,
    /// Funcion i: admite `"use strict"` (cuerpo, parametros simples, no estricta ya).
    fns: Vec<bool>,
    /// Profundidad de contexto estricto (clase, funcion o programa con la directiva).
    strict_ctx: usize,
}

impl Mutator {
    fn new(op: Option<Op>) -> Mutator {
        Mutator {
            op,
            stmts: 0,
            lits: Vec::new(),
            hit: None,
            sites: Vec::new(),
            vars: Vec::new(),
            fns: Vec::new(),
            strict_ctx: 0,
        }
    }

    /// M33: si `op` apunta a la sentencia `id` con un operador nuevo, marca el
    /// sitio y lo devuelve para aplicarlo despues de recorrerla (asi la
    /// numeracion de lo que tiene adentro no cambia).
    fn stmt_extra(&mut self, id: usize, s: &Stmt) -> Option<Extra> {
        match self.op {
            Some(Op::X(x @ (Extra::Wrap(i, _) | Extra::Dup(i) | Extra::If(i, _)))) if i == id => {
                self.hit = Some(s.span());
                Some(x)
            }
            _ => None,
        }
    }
}

/// M33: aplica un operador de sentencia. `true` si hay que duplicarla (lo hace
/// el llamador, que tiene la lista).
fn apply_stmt_extra(x: Extra, s: &mut Stmt) -> bool {
    match x {
        Extra::Wrap(_, WrapTo::Block) => {
            let old = std::mem::take(s);
            *s = Stmt::Block(BlockStmt { stmts: vec![old], ..Default::default() });
        }
        Extra::Wrap(_, WrapTo::Iife) => {
            let old = std::mem::take(s);
            let arrow = Expr::Arrow(ArrowExpr {
                body: Box::new(ArrowFunctionBody::FunctionBody(FunctionBody { span: DUMMY_SP, stmts: vec![old] })),
                ..Default::default()
            });
            let call = Expr::Call(CallExpr {
                callee: Callee::Expr(Box::new(Expr::Paren(ParenExpr { span: DUMMY_SP, expr: Box::new(arrow) }))),
                ..Default::default()
            });
            *s = Stmt::Expr(ExprStmt { span: DUMMY_SP, expr: Box::new(call) });
        }
        Extra::Dup(_) => return true,
        Extra::If(_, to) => {
            if let Stmt::If(f) = s {
                match to {
                    IfTo::Negate => {
                        let test = std::mem::take(&mut f.test);
                        *f.test = Expr::Unary(UnaryExpr {
                            span: DUMMY_SP,
                            op: UnaryOp::Bang,
                            arg: Box::new(Expr::Paren(ParenExpr { span: DUMMY_SP, expr: test })),
                        });
                    }
                    IfTo::Swap => {
                        if let Some(alt) = f.alt.take() {
                            let cons = std::mem::replace(&mut f.cons, alt);
                            f.alt = Some(cons);
                        }
                    }
                }
            }
        }
        Extra::Neg(..) | Extra::Var(..) | Extra::Strict(_) => {}
    }
    false
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
        let mut dup = None;
        let prologue = n.iter().take_while(|s| is_directive(s)).count();
        for (i, s) in n.iter_mut().enumerate().skip(prologue) {
            let id = self.stmts;
            self.stmts += 1;
            self.sites.push(stmt_site(s));
            if self.op == Some(Op::Delete(id)) {
                doomed = Some(i);
                self.hit = Some(s.span());
            }
            let extra = self.stmt_extra(id, s);
            s.visit_mut_with(self);
            if extra.is_some_and(|x| apply_stmt_extra(x, s)) {
                dup = Some(i);
            }
        }
        if let Some(i) = doomed {
            if !strip_init(&mut n[i]) {
                n.remove(i);
            }
        }
        if let Some(i) = dup {
            let c = n[i].clone();
            n.insert(i + 1, c);
        }
    }

    /// M33: censo de declaraciones (`var` <-> `let`/`const`).
    fn visit_mut_var_decl(&mut self, v: &mut VarDecl) {
        let id = self.vars.len();
        self.vars.push(VarSite {
            kind: v.kind,
            all_init: v.decls.iter().all(|d| d.init.is_some()),
        });
        let to = match self.op {
            Some(Op::X(Extra::Var(i, to))) if i == id => {
                self.hit = Some(v.span);
                Some(to)
            }
            _ => None,
        };
        v.visit_mut_children_with(self);
        if let Some(to) = to {
            v.kind = to;
        }
    }

    /// M33: censo de funciones para `"use strict"`. Las de una clase o de un
    /// contexto ya estricto no cuentan como candidatas (no cambiaria nada).
    fn visit_mut_function(&mut self, f: &mut Function) {
        let id = self.fns.len();
        let own = f.body.as_ref().is_some_and(|b| b.stmts.iter().take_while(|s| is_directive(s)).any(is_use_strict));
        let simple = f.params.iter().all(|p| matches!(p.pat, Pat::Ident(_)));
        self.fns.push(f.body.is_some() && simple && !own && self.strict_ctx == 0);
        let apply = self.op == Some(Op::X(Extra::Strict(Some(id))));
        if apply {
            self.hit = Some(f.span);
        }
        if own {
            self.strict_ctx += 1;
        }
        f.visit_mut_children_with(self);
        if own {
            self.strict_ctx -= 1;
        }
        if apply {
            if let Some(b) = f.body.as_mut() {
                b.stmts.insert(0, use_strict());
            }
        }
    }

    fn visit_mut_class(&mut self, c: &mut Class) {
        self.strict_ctx += 1;
        c.visit_mut_children_with(self);
        self.strict_ctx -= 1;
    }

    /// M15: la lista top-level de un modulo. Solo se borran sentencias; los
    /// import/export no (borrarlos rompe el enlace o la forma del modulo), pero
    /// se recorren (un `export function` tiene sentencias adentro).
    fn visit_mut_module_items(&mut self, n: &mut Vec<ModuleItem>) {
        let mut doomed = None;
        let mut dup = None;
        let prologue = n.iter().take_while(|it| matches!(it, ModuleItem::Stmt(s) if is_directive(s))).count();
        for (i, item) in n.iter_mut().enumerate().skip(prologue) {
            let mut extra = None;
            if let ModuleItem::Stmt(s) = item {
                let id = self.stmts;
                self.stmts += 1;
                self.sites.push(stmt_site(s));
                if self.op == Some(Op::Delete(id)) {
                    doomed = Some(i);
                    self.hit = Some(s.span());
                }
                extra = self.stmt_extra(id, s);
            }
            item.visit_mut_with(self);
            if let (Some(x), ModuleItem::Stmt(s)) = (extra, &mut *item) {
                if apply_stmt_extra(x, s) {
                    dup = Some(i);
                }
            }
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
        if let Some(i) = dup {
            let c = n[i].clone();
            n.insert(i + 1, c);
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
        // M33: `n` → `-n` / `-1` sobre el mismo censo de literales.
        if let (Some(Op::X(Extra::Neg(i, to))), Site::Num(v)) = (self.op, &site) {
            if i == id {
                self.hit = Some(n.span());
                *n = to.expr(*v);
            }
        }
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
#[cfg(test)]
pub fn variants_for(src: &str, n: usize, seed: u64, kind: SourceKind) -> Option<Vec<Variant>> {
    variants_ext(src, n, 0, seed, kind)
}

/// Las `n` clasicas (identicas a las de siempre) y despues hasta `extra` de los
/// operadores nuevos (M33), con su propio PRNG y sin repetir texto.
pub fn variants_ext(src: &str, n: usize, extra: usize, seed: u64, kind: SourceKind) -> Option<Vec<Variant>> {
    if n == 0 && extra == 0 {
        return Some(Vec::new());
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || variants_inner(src, n, extra, seed, kind))
    }))
    .unwrap_or(Some(Vec::new()))
}

/// M33: los operadores nuevos que admite el programa, por familia (en el orden
/// del consolidado). Se reparten en ronda: con pocas variantes igual sale una
/// de cada familia.
fn extra_ops(census: &Mutator, kind: SourceKind) -> Vec<Vec<Op>> {
    let x = |e| Op::X(e);
    let mut neg = Vec::new();
    for (i, site) in census.lits.iter().enumerate() {
        if let Site::Num(v) = site {
            if *v != 0.0 && *v != 1.0 && v.is_finite() {
                neg.push(x(Extra::Neg(i, NegTo::Opp)));
            }
            neg.push(x(Extra::Neg(i, NegTo::MinusOne)));
        }
    }
    let mut wrap = Vec::new();
    let mut dup = Vec::new();
    let mut ifs = Vec::new();
    for (i, s) in census.sites.iter().enumerate() {
        if s.block {
            wrap.push(x(Extra::Wrap(i, WrapTo::Block)));
        }
        if s.iife {
            wrap.push(x(Extra::Wrap(i, WrapTo::Iife)));
        }
        if s.dup {
            dup.push(x(Extra::Dup(i)));
        }
        if let Some(alt) = s.if_alt {
            ifs.push(x(Extra::If(i, IfTo::Negate)));
            if alt {
                ifs.push(x(Extra::If(i, IfTo::Swap)));
            }
        }
    }
    let mut vars = Vec::new();
    for (i, v) in census.vars.iter().enumerate() {
        match v.kind {
            VarDeclKind::Var => {
                vars.push(x(Extra::Var(i, VarDeclKind::Let)));
                if v.all_init {
                    vars.push(x(Extra::Var(i, VarDeclKind::Const)));
                }
            }
            _ => vars.push(x(Extra::Var(i, VarDeclKind::Var))),
        }
    }
    // Solo carril A: un modulo ya es estricto.
    let mut strict = Vec::new();
    if kind == SourceKind::Script {
        if census.strict_ctx == 0 {
            strict.push(x(Extra::Strict(None)));
        }
        strict.extend(census.fns.iter().enumerate().filter(|(_, ok)| **ok).map(|(i, _)| x(Extra::Strict(Some(i)))));
    }
    vec![neg, wrap, dup, ifs, vars, strict]
}

/// El programa empieza con `"use strict"` (el censo arranca en contexto estricto).
fn top_strict(p: &Program) -> bool {
    match p {
        Program::Script(s) => s.body.iter().take_while(|s| is_directive(s)).any(is_use_strict),
        Program::Module(_) => true,
    }
}

fn variants_inner(src: &str, n: usize, extra: usize, seed: u64, kind: SourceKind) -> Option<Vec<Variant>> {
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
            extra: false,
            tdz_filter: false,
        });
    }

    // M33: los operadores nuevos, despues y con otro PRNG: las clasicas de
    // arriba no cambian por pedir extras.
    if extra > 0 {
        let mut census = Mutator::new(None);
        census.strict_ctx = usize::from(top_strict(&program));
        program.clone().visit_mut_with(&mut census);
        let mut fams = extra_ops(&census, kind);
        let mut rng = Rng(seed ^ 0x4d33_6d75_7461_7221);
        for f in &mut fams {
            rng.shuffle(f);
            f.reverse();
        }
        let mut made = 0;
        let mut turn = 0usize;
        while made < extra && fams.iter().any(|f| !f.is_empty()) {
            let fam = turn % fams.len();
            turn += 1;
            let Some(op) = fams[fam].pop() else {
                continue;
            };
            let mut p = program.clone();
            let mut m = Mutator::new(Some(op));
            m.strict_ctx = usize::from(top_strict(&program));
            p.visit_mut_with(&mut m);
            if op == Op::X(Extra::Strict(None)) {
                if let Program::Script(s) = &mut p {
                    s.body.insert(0, use_strict());
                    m.hit = Some(s.span);
                }
            }
            let Some(hit) = m.hit else {
                continue;
            };
            let Some(text) = print(&cm, p) else {
                continue;
            };
            if seen.contains(&text) || parse(&Lrc::new(SourceMap::default()), &text, kind).is_none() {
                continue;
            }
            seen.insert(text.clone());
            made += 1;
            out.push(Variant {
                desc: describe(&cm, op, hit),
                src: text,
                extra: true,
                tdz_filter: matches!(op, Op::X(Extra::Var(..))),
            });
        }
    }
    Some(out)
}

fn describe(cm: &SourceMap, op: Op, hit: Span) -> String {
    if op == Op::X(Extra::Strict(None)) {
        return "[extra] \"use strict\" al principio del programa".into();
    }
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
        Op::X(x) => match x {
            Extra::Neg(i, NegTo::Opp) => format!("[extra] literal #{i} (linea {line}): {snippet} -> -{snippet}"),
            Extra::Neg(i, NegTo::MinusOne) => format!("[extra] literal #{i} (linea {line}): {snippet} -> -1"),
            Extra::Wrap(i, WrapTo::Block) => format!("[extra] envolver sentencia #{i} en un bloque (linea {line}): {snippet}"),
            Extra::Wrap(i, WrapTo::Iife) => format!("[extra] envolver sentencia #{i} en una IIFE flecha (linea {line}): {snippet}"),
            Extra::Dup(i) => format!("[extra] duplicar sentencia #{i} (linea {line}): {snippet}"),
            Extra::If(i, IfTo::Negate) => format!("[extra] negar la condicion del if (sentencia #{i}, linea {line}): {snippet}"),
            Extra::If(i, IfTo::Swap) => format!("[extra] intercambiar las ramas del if (sentencia #{i}, linea {line}): {snippet}"),
            Extra::Var(i, to) => format!("[extra] declaracion #{i} (linea {line}): {snippet} -> {to:?}"),
            Extra::Strict(Some(i)) => format!("[extra] \"use strict\" en la funcion #{i} (linea {line}): {snippet}"),
            Extra::Strict(None) => unreachable!("se describe arriba"),
        },
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

    /// M33: pedir extras no cambia las clasicas (son un prefijo identico), y las
    /// extras salen despues, marcadas y deterministas.
    #[test]
    fn extras_no_cambian_las_clasicas() {
        use super::variants_ext;
        use crate::minify::SourceKind;
        let classic: Vec<String> = variants(SRC, 6, 7).into_iter().map(|v| v.src).collect();
        let ext = variants_ext(SRC, 6, 8, 7, SourceKind::Script).unwrap();
        let (a, b): (Vec<_>, Vec<_>) = ext.iter().partition(|v| !v.extra);
        assert_eq!(a.iter().map(|v| v.src.clone()).collect::<Vec<_>>(), classic);
        assert_eq!(b.len(), 8);
        assert!(ext.iter().take(6).all(|v| !v.extra) && ext.iter().skip(6).all(|v| v.extra));
        let again: Vec<String> = variants_ext(SRC, 6, 8, 7, SourceKind::Script).unwrap().into_iter().map(|v| v.src).collect();
        assert_eq!(again, ext.iter().map(|v| v.src.clone()).collect::<Vec<_>>());
        // Solo extras.
        assert!(variants_ext(SRC, 0, 3, 7, SourceKind::Script).unwrap().iter().all(|v| v.extra));
    }

    /// M33: cada operador nuevo genera lo que dice, y todo reparsea.
    #[test]
    fn operadores_nuevos() {
        use super::variants_ext;
        use crate::minify::SourceKind;
        let src = "var k = 5;\nlet four = 4;\nfunction f(x) {\n  if (x) {\n    k = 2;\n  } else k = 3;\n  return k;\n}\nconsole.log(f(1), [2][0]);\n";
        let all = variants_ext(src, 0, 200, 3, SourceKind::Script).unwrap();
        let has = |s: &str| all.iter().any(|v| v.src.contains(s));
        for v in &all {
            assert!(parses(&v.src), "no parsea: {}\n{}", v.desc, v.src);
            assert!(v.desc.starts_with("[extra] "), "{}", v.desc);
        }
        // Literal a negativo: `-n` y `-1` (y en el 0 de `[0]`, solo `-1`).
        for want in ["var k = -5;", "var k = -1;", "let four = -4;", "][-1]);"] {
            assert!(has(want), "falta {want}");
        }
        assert!(!has("][-0]"));
        // Envolver: bloque e IIFE flecha.
        assert!(has("    {\n        return k;\n    }\n"));
        assert!(has("(()=>{\n    console.log(f(1), [\n        2\n    ][0]);\n})();"));
        // Duplicar.
        assert!(has("        k = 2;\n        k = 2;\n"));
        assert!(has("var k = 5;\nvar k = 5;\n"));
        // if: negar e intercambiar.
        assert!(has("    if (!x) {\n        k = 2;\n    } else k = 3;"));
        assert!(has("    if (x) k = 3;\n    else {\n        k = 2;\n    }"));
        // var <-> let/const.
        for want in ["let k = 5;", "const k = 5;", "var four = 4;"] {
            assert!(has(want), "falta {want}");
        }
        // "use strict": al principio y en la funcion.
        assert!(has("\"use strict\";\nvar k = 5;"));
        assert!(has("function f(x) {\n    \"use strict\";\n"));
        // El filtro tdz va solo en var <-> let/const.
        assert!(all.iter().all(|v| v.tdz_filter == v.desc.contains("declaracion #")));
        // Un `let` no se envuelve en un bloque ni se duplica; una funcion no va a un bloque.
        assert!(!all.iter().any(|v| v.src.contains("{\n    let four")));
        assert!(!all.iter().any(|v| v.src.contains("{\n    function f")));
        // Carril B: sin "use strict" (un modulo ya es estricto).
        let esm = variants_ext("export var a = 2;\nif (a) console.log(a);\n", 0, 50, 3, SourceKind::Module).unwrap();
        assert!(!esm.is_empty() && !esm.iter().any(|v| v.desc.contains("use strict")));
        assert!(esm.iter().any(|v| v.src.contains("export let a = 2;")));
        // Ya estricto: no se agrega.
        let st = variants_ext("\"use strict\";\nfunction g() { return 1; }\nconsole.log(g());\n", 0, 50, 3, SourceKind::Script).unwrap();
        assert!(!st.iter().any(|v| v.desc.contains("use strict")));
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
