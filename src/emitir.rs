//! M29: `difuzz --emitir <archivo.js> [--minifier/--config/--forma/--carril]`.
//! Imprime el original y el minificado de `minify::prepare`, sin correr node.
//! Sirve para auditar falsos negativos contra el `output.js` de un fixture y
//! para reducir a mano. Minifica en proceso (sin hijo): es un solo caso.

use std::path::Path;

use crate::backend::Minifier;
use crate::minify::{self, Forma, Outcome, Preset, SourceKind};

/// El texto a imprimir, o el motivo por el que no hay minificado.
pub fn emitir(path: &Path, minifier: Minifier, preset: Preset, forma: Forma, kind: SourceKind) -> Result<String, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("no pude leer {}: {e}", path.display()))?;
    let src = forma.apply(&raw);
    let celda = format!("{} {} {} carril {}", minifier.name(), preset.name(), forma.name(), kind.carril());
    // La misma pila que un worker (M1): lo que pasa en una corrida pasa aca.
    let outcome = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(crate::WORKER_STACK)
            .spawn_scoped(s, || minify::prepare(&src, &preset.build(minifier, kind)))
            .map_err(|e| format!("no pude crear el hilo: {e}"))?
            .join()
            .map_err(|_| "panic fuera del minifier".to_string())
    })?;
    match outcome {
        Outcome::Ok { original, minified, .. } => Ok(format!(
            "// difuzz --emitir {} ({celda})\n// ---- original\n{}// ---- minificado\n{}",
            path.display(),
            con_salto(original),
            con_salto(minified)
        )),
        Outcome::ParseError { esm, script } => Err(format!(
            "no parsea en el carril {} ({celda}){}",
            kind.carril(),
            if esm {
                ": parsea como modulo, probar --carril b"
            } else if script {
                ": parsea como script, probar --carril a"
            } else {
                ""
            }
        )),
        Outcome::CodegenError => Err(format!("error de codegen ({celda})")),
        Outcome::Panic(msg) => Err(format!("panic del minifier ({celda}):\n{msg}")),
    }
}

fn con_salto(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aceptacion de M29 con el reducido de O1, el Issue C (oxc#27863) (reconstruido: el reducido original
    /// no se conservo).
    #[test]
    fn emite_o1_de_oxc() {
        let d = std::env::temp_dir().join(format!("difuzz-test-emitir-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("o1.mjs");
        std::fs::write(&f, "const __proto__ = 0;\nconsole.log(Object.keys({ __proto__ }));\n").unwrap();
        let out = emitir(&f, Minifier::Oxc, Preset::CarrilA, Forma::TalCual, SourceKind::Module).unwrap();
        assert!(out.contains("// ---- original\n") && out.contains("// ---- minificado\n"), "{out}");
        let min = out.split("// ---- minificado\n").nth(1).unwrap();
        assert!(min.contains("Object.keys({0})"), "{out}");
        // Un script con `import` no parsea en el carril A y lo dice.
        std::fs::write(&f, "import x from 'y';\n").unwrap();
        let e = emitir(&f, Minifier::Swc, Preset::CarrilA, Forma::TalCual, SourceKind::Script).unwrap_err();
        assert!(e.contains("probar --carril b"), "{e}");
        assert!(emitir(&d.join("no-existe.js"), Minifier::Swc, Preset::CarrilA, Forma::TalCual, SourceKind::Script).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
