//! Chequeos estaticos sobre el AST, sin ejecutar nada.

use swc_ecma_ast::{BindingIdent, Expr, FnDecl, Ident, MemberExpr, Program};
use swc_ecma_visit::{Visit, VisitWith};

/// Asunciones de AGENTS.md que se ven en el fuente y no en runtime:
/// `arguments.callee`, `.caller` y bindings llamados `undefined`, `NaN` o
/// `Infinity`. Se suman a los flags del preload (known.rs).
pub fn static_flags(program: &Program) -> Vec<&'static str> {
    let mut v = StaticFlags::default();
    program.visit_with(&mut v);
    let mut out = Vec::new();
    if v.callee {
        out.push("callee");
    }
    if v.redefines_global {
        out.push("redefine_global");
    }
    out
}

#[derive(Default)]
struct StaticFlags {
    callee: bool,
    redefines_global: bool,
}

impl StaticFlags {
    fn check_name(&mut self, id: &Ident) {
        if matches!(id.sym.as_str(), "undefined" | "NaN" | "Infinity") {
            self.redefines_global = true;
        }
    }
}

impl Visit for StaticFlags {
    fn visit_member_expr(&mut self, n: &MemberExpr) {
        let on_arguments = matches!(&*n.obj, Expr::Ident(i) if i.sym.as_str() == "arguments");
        if n.prop.is_ident_with("caller") || (on_arguments && n.prop.is_ident_with("callee")) {
            self.callee = true;
        }
        n.visit_children_with(self);
    }

    // Declaraciones, parametros, catch y destinos de asignacion simples.
    fn visit_binding_ident(&mut self, n: &BindingIdent) {
        self.check_name(&n.id);
        n.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, n: &FnDecl) {
        self.check_name(&n.ident);
        n.visit_children_with(self);
    }
}
