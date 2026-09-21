//! CMP: que cuenta como divergencia (delta v1.2 §1 §3.4) y como se guarda.
//!
//! Precondicion que garantiza main: el original termino con exit 0 y stdout
//! no vacio (el guard de salida vacia de tests/exec.rs).

use std::path::{Path, PathBuf};

use crate::runner::{Exec, Status};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// El hallazgo clasico: mismo exit, distinto stdout.
    Stdout,
    /// El minificado lanzo (o murio por senal) y el original no.
    ExitStatus,
    /// El minificado se colgo y el original no. Posible loop introducido.
    Timeout,
}

pub enum Verdict {
    Same,
    Divergence(Kind),
}

pub fn compare(a: &Exec, b: &Exec) -> Verdict {
    match b.status {
        Status::Timeout => return Verdict::Divergence(Kind::Timeout),
        Status::Exited(0) => {}
        Status::Exited(_) | Status::Signal => return Verdict::Divergence(Kind::ExitStatus),
        // Error del fuzzer, no del programa: no es divergencia. main lo cuenta.
        Status::Failed(_) => return Verdict::Same,
    }
    if a.stdout != b.stdout {
        Verdict::Divergence(Kind::Stdout)
    } else {
        Verdict::Same
    }
}

/// Un directorio por hallazgo con todo lo necesario para reproducirlo a mano.
pub struct Reporter {
    root: PathBuf,
    config_name: &'static str,
}

impl Reporter {
    pub fn new(root: &Path, config_name: &'static str) -> Reporter {
        Reporter {
            root: root.to_path_buf(),
            config_name,
        }
    }

    pub fn divergence(
        &self,
        case: &Path,
        original: &str,
        minified: &str,
        a: &Exec,
        b: &Exec,
        kind: Kind,
    ) {
        let dir = self.case_dir("div", case);
        let meta = format!(
            "case: {}\nkind: {kind:?}\nconfig: {}\n\noriginal: {:?} en {:?}\nminificado: {:?} en {:?}\n",
            case.display(),
            self.config_name,
            a.status,
            a.elapsed,
            b.status,
            b.elapsed,
        );
        write_all(
            &dir,
            &[
                ("meta.txt", meta.as_str()),
                ("original.js", original),
                ("minified.js", minified),
                ("a.stdout", a.stdout.as_str()),
                ("a.stderr", a.stderr.as_str()),
                ("b.stdout", b.stdout.as_str()),
                ("b.stderr", b.stderr.as_str()),
            ],
        );
    }

    pub fn panic(&self, case: &Path, src: &str, message: &str) {
        let dir = self.case_dir("panic", case);
        let meta = format!(
            "case: {}\nkind: Panic\nconfig: {}\n\n{message}\n",
            case.display(),
            self.config_name
        );
        write_all(&dir, &[("meta.txt", meta.as_str()), ("input.js", src)]);
    }

    /// `<root>/<prefijo>/<ruta del caso con '/' → '__'>`
    fn case_dir(&self, prefix: &str, case: &Path) -> PathBuf {
        let slug: Vec<String> = case
            .parent()
            .unwrap_or(case)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|c| c != "." && c != "/")
            .collect();
        self.root.join(prefix).join(slug.join("__"))
    }
}

fn write_all(dir: &Path, files: &[(&str, &str)]) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("reporter: no pude crear {}: {e}", dir.display());
        return;
    }
    for (name, content) in files {
        if let Err(e) = std::fs::write(dir.join(name), content) {
            eprintln!("reporter: no pude escribir {}/{name}: {e}", dir.display());
        }
    }
}
