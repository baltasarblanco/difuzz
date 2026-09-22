//! EXEC: runner propio sobre `std::process`. Reemplaza a `exec_node_js`, que
//! no tiene timeout, mezcla stdout con stderr en el error y no expone el exit
//! code (delta v1.2 §1 §3.3).
//!
//! Carril A: el programa se escribe a un `.cjs` y se corre con `node archivo`.
//! Node lo envuelve en la funcion de CommonJS: sloppy mode, top-level privado.
//!
//! A y B corren con el mismo path, el mismo preload y TZ=UTC: cualquier cosa
//! que el programa pueda leer del entorno es identica en los dos lados.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Determinismo + observador. Ver `preload.js`.
pub const PRELOAD: &str = include_str!("preload.js");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Exited(i32),
    /// Termino por senal (sin exit code). En Linux: crash, OOM killer, etc.
    Signal,
    /// Supero el timeout y fue matado.
    Timeout,
    /// No se pudo escribir el archivo o lanzar `node`. Error del fuzzer, no del
    /// programa: main lo trata aparte.
    Failed(String),
}

pub struct Exec {
    pub status: Status,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

impl Exec {
    pub fn ok_with_output(&self) -> bool {
        self.status == Status::Exited(0) && !self.stdout.trim().is_empty()
    }

    pub fn ok_silent(&self) -> bool {
        self.status == Status::Exited(0) && self.stdout.trim().is_empty()
    }

    fn failed(msg: String, start: Instant) -> Exec {
        Exec {
            status: Status::Failed(msg),
            stdout: String::new(),
            stderr: String::new(),
            elapsed: start.elapsed(),
        }
    }
}

/// El preload escrito a disco una vez por proceso; lo comparten todos los
/// runners y se borra al soltar el guard.
pub struct Preload {
    path: PathBuf,
}

impl Preload {
    pub fn write() -> std::io::Result<Preload> {
        let path = std::env::temp_dir().join(format!("difuzz-{}-preload.cjs", std::process::id()));
        std::fs::write(&path, PRELOAD)?;
        Ok(Preload { path })
    }
}

impl Drop for Preload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct Runner {
    /// Un solo path por worker: A y B de un mismo caso corren en secuencia en
    /// el mismo worker, y un programa que imprime su propio filename
    /// (`module`, stacks) tiene que ver lo mismo en los dos lados.
    script: PathBuf,
    preload: PathBuf,
    timeout: Duration,
}

impl Runner {
    pub fn new(preload: &Preload, worker: usize, timeout: Duration) -> Runner {
        Runner {
            script: std::env::temp_dir()
                .join(format!("difuzz-{}-w{worker}.cjs", std::process::id())),
            preload: preload.path.clone(),
            timeout,
        }
    }

    pub fn run(&self, js: &str) -> Exec {
        let start = Instant::now();
        if let Err(e) = std::fs::write(&self.script, js) {
            return Exec::failed(format!("escribir {}: {e}", self.script.display()), start);
        }
        run_file(&self.preload, &self.script, self.timeout, start)
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.script);
    }
}

fn run_file(preload: &Path, script: &Path, timeout: Duration, start: Instant) -> Exec {
    let mut child = match Command::new("node")
        .arg("--require")
        .arg(preload)
        .arg(script)
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Exec::failed(format!("spawn node: {e}"), start),
    };

    // Los pipes se drenan en hilos propios. Si no, un programa que imprime mas
    // que el buffer del pipe se bloquea escribiendo y nosotros esperando: deadlock
    // que el timeout reportaria como "cuelgue", falso.
    let mut out_pipe = child.stdout.take().expect("stdout fue pedido como piped");
    let mut err_pipe = child.stderr.take().expect("stderr fue pedido como piped");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                break match st.code() {
                    Some(code) => Status::Exited(code),
                    None => Status::Signal,
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Status::Timeout;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Status::Failed(format!("wait: {e}"));
            }
        }
    };

    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();

    Exec {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        elapsed: start.elapsed(),
    }
}
