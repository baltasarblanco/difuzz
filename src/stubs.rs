//! M30 (`--stubs`, opt-in): cobertura del carril B para modulos cuyo `import`
//! no resuelve (hoy descarte `enlace`).
//!
//! El preload registra hooks de loader de node (`module.registerHooks`,
//! sincronicos y en el mismo hilo; MEDIDO con node v24.14.1: andan con `import`
//! estatico y dinamico, nombres de export string y `require`) cuando recibe
//! `DIFUZZ_STUBS=<manifiesto>`. Un specifier que no resuelve
//! (ERR_MODULE_NOT_FOUND y afines) pasa a un modulo stub: ESM con `default` y
//! cada nombre pedido, o CJS con `module.exports`. Todos los valores son el
//! mismo Proxy inerte del preload (funcion que devuelve stubs, propiedades que
//! son stubs, `then` undefined, primitivo `0` / `"[stub <spec>]"`).
//!
//! Los nombres que un ESM tiene que exportar no se pueden adivinar en el
//! loader: salen de este manifiesto, calculado con SWC sobre el ORIGINAL de
//! cada caso y escrito una vez antes de correr A. B corre con el mismo archivo:
//! el stub es identico en A y en B aunque el minifier toque los imports.

use std::collections::BTreeMap;

use swc_common::{Globals, GLOBALS};
use swc_ecma_ast::{ExportSpecifier, ImportSpecifier, ModuleDecl, ModuleExportName, ModuleItem, Program};

use crate::minify::SourceKind;

/// `{"<specifier>": ["<nombre>", ...], ...}`: por cada `import ... from` y
/// `export ... from` del programa, los nombres que pide (`default` incluido).
/// Un namespace (`* as ns`) o `export *` no pide nombres: el stub igual existe.
/// En el carril A (script) no hay imports estaticos: `{}`.
pub fn manifest(src: &str, kind: SourceKind) -> String {
    let names = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || collect(src, kind))
    }))
    .unwrap_or_default();
    let mut out = String::from("{");
    for (i, (spec, names)) in names.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_str(spec));
        out.push_str(":[");
        let list: Vec<String> = names.iter().map(|n| json_str(n)).collect();
        out.push_str(&list.join(","));
        out.push(']');
    }
    out.push('}');
    out
}

fn collect(src: &str, kind: SourceKind) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if kind != SourceKind::Module {
        return map;
    }
    let Some(Program::Module(m)) = crate::backend::swc::parse_for(src, kind) else {
        return map;
    };
    let name = |n: &ModuleExportName| match n {
        ModuleExportName::Ident(i) => i.sym.to_string(),
        ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
    };
    let mut add = |spec: String, n: Option<String>| {
        let e = map.entry(spec).or_default();
        if let Some(n) = n {
            if !e.contains(&n) {
                e.push(n);
            }
        }
    };
    for item in &m.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(i) => {
                let spec = i.src.value.to_string_lossy().into_owned();
                add(spec.clone(), None);
                for s in &i.specifiers {
                    let n = match s {
                        ImportSpecifier::Named(n) => Some(n.imported.as_ref().map_or_else(|| n.local.sym.to_string(), name)),
                        ImportSpecifier::Default(_) => Some("default".to_string()),
                        ImportSpecifier::Namespace(_) => None,
                    };
                    add(spec.clone(), n);
                }
            }
            ModuleDecl::ExportNamed(e) => {
                let Some(src) = &e.src else {
                    continue;
                };
                let spec = src.value.to_string_lossy().into_owned();
                add(spec.clone(), None);
                for s in &e.specifiers {
                    if let ExportSpecifier::Named(n) = s {
                        add(spec.clone(), Some(name(&n.orig)));
                    }
                }
            }
            ModuleDecl::ExportAll(e) => add(e.src.value.to_string_lossy().into_owned(), None),
            _ => {}
        }
    }
    map
}

/// String JSON con escapes (comillas, barra, control y separadores de linea).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::manifest;
    use crate::minify::SourceKind;

    #[test]
    fn manifiesto_de_imports() {
        let src = "import d, { a, b as c, \"x-y\" as z } from './m.mjs';\nimport * as ns from 'pkg';\nimport './solo.mjs';\nexport { q, default as r } from './m.mjs';\nexport * from 'otro';\nexport const k = 1;\n";
        assert_eq!(
            manifest(src, SourceKind::Module),
            "{\"./m.mjs\":[\"default\",\"a\",\"b\",\"x-y\",\"q\"],\"./solo.mjs\":[],\"otro\":[],\"pkg\":[]}"
        );
        // Script: no hay imports estaticos; uno que no parsea, vacio.
        assert_eq!(manifest("require('x');\n", SourceKind::Script), "{}");
        assert_eq!(manifest("import {", SourceKind::Module), "{}");
        assert_eq!(super::json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }

    /// Node de verdad: con stubs, lo que no enlazaba corre, sin prender `proxy`;
    /// sin stubs sigue siendo enlace.
    #[test]
    fn stubs_en_node() {
        use crate::runner::{Preload, Runner, Status, LINK_EXIT};
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let preload = Preload::write_tagged("m30").unwrap();
        let js = "import d, { a, \"x-y\" as z } from './no-existe.mjs';\nimport * as ns from 'pkg-que-no-existe';\nconst dyn = await import('./otro-no.mjs');\nconsole.log(typeof d, typeof a, String(a.b.c), +a, a === a, a.b === a.b, Object.keys(ns).join(), z() === z(), typeof dyn.default, a.then);\na.k = 5;\nconsole.log(a.k, [...a].length, new d() === new d());\n";
        let off = Runner::new(&preload, 9988, std::time::Duration::from_secs(20)).with_kind(SourceKind::Module);
        assert_eq!(off.run(js).status, Status::Exited(LINK_EXIT));
        let r = Runner::new(&preload, 9987, std::time::Duration::from_secs(20)).with_kind(SourceKind::Module).with_stubs(true);
        r.set_stubs(&manifest(js, SourceKind::Module));
        let e = r.run(js);
        assert_eq!(e.status, Status::Exited(0), "{}\n{}", e.stdout, e.stderr);
        assert_eq!(e.stdout, "function function [stub ./no-existe.mjs.a.b.c] 0 true true default true function undefined\n5 0 true\n");
        assert!(e.flags.is_empty(), "{:?}", e.flags);
        // Carril A: require de algo que no existe.
        let ra = Runner::new(&preload, 9986, std::time::Duration::from_secs(20)).with_stubs(true);
        ra.set_stubs(&manifest("", SourceKind::Script));
        let e = ra.run("const m = require('./no-existe-cjs');\nconsole.log(typeof m, String(m.x), m.__esModule);\n");
        assert_eq!((e.status, e.stdout.as_str()), (Status::Exited(0), "function [stub ./no-existe-cjs.x] undefined\n"));
        // Un import que si existe no se toca.
        let e = r.run("import { readFileSync } from 'node:fs';\nconsole.log(typeof readFileSync);\n");
        assert_eq!(e.stdout, "function\n");
    }
}
