//! Chequeos estaticos sobre el AST, sin ejecutar nada.

use swc_ecma_ast::{
    AssignExpr, AssignTarget, BindingIdent, Expr, FnDecl, Ident, MemberExpr, Pat, Program, Prop,
    PropName, PropOrSpread, SimpleAssignTarget, UpdateExpr,
};
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

/// Escrituras sobre un objeto recien creado (`[a, b][0] = x`, `({}).k = x`,
/// `[[a][0]] = …`): nadie puede leerlas. Es la firma del bug de swc#12397 en el
/// fixture 9739, que el observador no ve porque agrega un uso y apaga el
/// disparador. main compara la cuenta del original contra la del minificado:
/// si sube, es sospechoso (no veredicto: si nadie lee el destino, puede ser
/// equivalente).
pub fn dead_writes(program: &Program) -> usize {
    let mut v = DeadWrites(0);
    program.visit_with(&mut v);
    v.0
}

struct DeadWrites(usize);

impl Visit for DeadWrites {
    fn visit_assign_expr(&mut self, n: &AssignExpr) {
        if let AssignTarget::Simple(t) = &n.left {
            let hit = match t {
                SimpleAssignTarget::Member(m) => is_fresh(&m.obj),
                SimpleAssignTarget::Paren(p) => is_write_to_fresh(&p.expr),
                _ => false,
            };
            if hit {
                self.0 += 1;
            }
        }
        n.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, n: &UpdateExpr) {
        if is_write_to_fresh(&n.arg) {
            self.0 += 1;
        }
        n.visit_children_with(self);
    }

    // Destinos de destructuring y de for-in/of.
    fn visit_pat(&mut self, n: &Pat) {
        if let Pat::Expr(e) = n {
            if is_write_to_fresh(e) {
                self.0 += 1;
            }
        }
        n.visit_children_with(self);
    }
}

fn is_write_to_fresh(e: &Expr) -> bool {
    match e {
        Expr::Paren(p) => is_write_to_fresh(&p.expr),
        Expr::Member(m) => is_fresh(&m.obj),
        _ => false,
    }
}

/// Un literal que nadie mas referencia. Objetos solo si escribirles no puede
/// correr codigo ni cambiar el prototipo: sin getters/setters, sin `__proto__`,
/// sin claves string (podrian ser "__proto__").
fn is_fresh(e: &Expr) -> bool {
    match e {
        Expr::Paren(p) => is_fresh(&p.expr),
        Expr::Array(_) => true,
        Expr::Object(o) => o.props.iter().all(|p| match p {
            PropOrSpread::Spread(_) => true,
            PropOrSpread::Prop(p) => match &**p {
                Prop::Shorthand(i) => i.sym.as_str() != "__proto__",
                Prop::KeyValue(kv) => match &kv.key {
                    PropName::Ident(i) => i.sym.as_str() != "__proto__",
                    PropName::Str(_) => false,
                    _ => true,
                },
                Prop::Method(_) => true,
                _ => false,
            },
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{dead_writes, static_flags};
    use swc_common::{sync::Lrc, FileName, SourceMap};
    use swc_ecma_ast::{EsVersion, Program};
    use swc_ecma_parser::{parse_file_as_script, Syntax};

    fn parse(src: &str) -> Program {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
        let script =
            parse_file_as_script(&fm, Syntax::default(), EsVersion::latest(), None, &mut Vec::new())
                .unwrap();
        Program::Script(script)
    }

    #[test]
    fn cuenta_escrituras_sobre_literales() {
        assert_eq!(dead_writes(&parse("[1, 2][0] = 3;")), 1);
        assert_eq!(dead_writes(&parse("[[1][0], ({}).k] = [2, 3];")), 2);
        assert_eq!(dead_writes(&parse("[1][0]++; ({a: 1}).a += 1;")), 2);
        assert_eq!(dead_writes(&parse("var a = [1]; a[0] = 2; [a[0]] = [3];")), 0);
        assert_eq!(dead_writes(&parse("({ set k(v) {} }).k = 1; ({ __proto__: p }).k = 1;")), 0);
    }

    #[test]
    fn flags_estaticos() {
        assert_eq!(static_flags(&parse("function f() { return arguments.callee }")), ["callee"]);
        assert_eq!(static_flags(&parse("f.caller;")), ["callee"]);
        assert_eq!(static_flags(&parse("function g(NaN) {}")), ["redefine_global"]);
        assert!(static_flags(&parse("x === undefined; o.callee;")).is_empty());
    }
}
