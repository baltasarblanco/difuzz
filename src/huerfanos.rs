//! Subreaper (pendiente del fuego, M2): un programa que lanza
//! `child_process.spawn(..., {detached: true})` hace `setsid` y escapa del grupo
//! de procesos que el runner mata (M2) y del timeout; con `stdio: 'inherit'`
//! ademas retiene los pipes y el runner espera READ_GRACE.
//!
//! En Linux difuzz se declara subreaper (`prctl(PR_SET_CHILD_SUBREAPER)`): los
//! descendientes huerfanos pasan a ser hijos de difuzz en vez de init. Despues
//! de cada ejecucion `cosechar` mata y cosecha a los hijos de difuzz que no son
//! suyos. Un hijo legitimo es:
//! - node del runner o el hijo que minifica (subproceso.rs): lider de su grupo
//!   (`process_group(0)`), en la sesion de difuzz;
//! - `node --check`, `node --version`: en el grupo y la sesion de difuzz.
//!
//! Huerfano = hijo de difuzz en otra sesion (hizo `setsid`: `detached`) o en un
//! grupo ajeno del que no es lider (quedo del grupo de un node ya muerto). Esos
//! nunca los espera un `Child::wait`, asi que cosecharlos (waitpid por pid, nunca
//! -1) no le roba el status a nadie. Mientras no se cosecha, el pid no se reusa:
//! leer /proc y matar no tiene carrera.
//!
//! No cubre: un descendiente que hace `setpgid(0,0)` sin `setsid` (queda lider
//! de su grupo en nuestra sesion, igual que un node legitimo). Fuera de Linux,
//! no hace nada (compila igual).
//!
//! R1-H19: con `--jobs N` todos los workers comparten el proceso y el
//! subreaper. Un nieto queda huerfano (adoptado por difuzz) aunque el node que
//! lo origino siga corriendo (p. ej. `sh -c '(sleep 1; ...) &'`: el `sh` sale y
//! su hijo pasa a difuzz). MEDIDO antes del arreglo: la cosecha de OTRO worker
//! lo mataba a mitad de la ejecucion y cambiaba la salida de ese programa
//! (test `no_mata_huerfanos_de_un_node_en_curso`). Ahora un huerfano no se toca
//! mientras su node este en curso: si su grupo es el de un node vivo (el lider
//! sigue siendo hijo de difuzz), o si su directorio de trabajo (lo hereda del
//! node: el cwd de cada runner es propio) es el de una ejecucion en curso
//! (`EnCurso`). No sirve el entorno: el preload borra `DIFUZZ_CTL`. Lo cosecha
//! su propio runner al terminar, o la cosecha final de main. Un huerfano que se
//! cambio de directorio y de grupo no se puede atribuir: se mata como antes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ACTIVO: AtomicBool = AtomicBool::new(false);

/// R1-H19: los directorios de trabajo de los node que estan corriendo ahora.
static EN_CURSO: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());

/// R1-H19: marca una ejecucion de node como en curso hasta que se suelta. El
/// runner la toma antes de lanzar node y la suelta despues de cosecharlo y
/// antes de su propia cosecha.
pub struct EnCurso(std::path::PathBuf);

pub fn en_curso(cwd: &std::path::Path) -> EnCurso {
    EN_CURSO.lock().unwrap_or_else(|e| e.into_inner()).push(cwd.to_path_buf());
    EnCurso(cwd.to_path_buf())
}

impl Drop for EnCurso {
    fn drop(&mut self) {
        let mut v = EN_CURSO.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = v.iter().position(|c| *c == self.0) {
            v.swap_remove(i);
        }
    }
}

/// R1-H19: `dir` esta dentro del directorio de trabajo de una ejecucion en curso.
#[cfg(target_os = "linux")]
fn cwd_en_curso(dir: &std::path::Path) -> bool {
    EN_CURSO.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|c| dir.starts_with(c))
}

/// Declara a difuzz subreaper. `true` si quedo activo.
pub fn activar() -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl con un entero; no toca memoria de este proceso.
        let r = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1 as libc::c_ulong, 0, 0, 0) };
        if r == 0 {
            ACTIVO.store(true, Ordering::SeqCst);
            return true;
        }
    }
    false
}

pub fn activo() -> bool {
    ACTIVO.load(Ordering::SeqCst)
}

/// Mata y cosecha los huerfanos adoptados. Devuelve cuantos cosecho. Sin
/// subreaper activo, no hace nada.
pub fn cosechar() -> usize {
    if !activo() {
        return 0;
    }
    #[cfg(target_os = "linux")]
    {
        linux::cosechar()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(target_os = "linux")]
mod linux {
    /// `(pgrp, session)` de `/proc/<pid>/stat` (campos 5 y 6, contando desde el
    /// que sigue al `)` final del nombre, que puede tener espacios).
    pub(super) fn grupo_y_sesion(pid: i32) -> Option<(i32, i32)> {
        let st = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let resto = &st[st.rfind(')')? + 1..];
        let mut f = resto.split_whitespace();
        // estado, ppid, pgrp, session
        let pgrp = f.nth(2)?.parse().ok()?;
        let sesion = f.next()?.parse().ok()?;
        Some((pgrp, sesion))
    }

    /// R1-H19: el lider del grupo `grupo` sigue siendo hijo nuestro (un node en
    /// curso, o recien terminado y sin cosechar: su runner matara el grupo).
    /// Un pgid en uso no se reasigna a otro proceso, asi que leerlo no tiene carrera.
    fn lider_vivo(grupo: i32, yo: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{grupo}/stat"))
            .ok()
            .and_then(|st| st[st.rfind(')')? + 1..].split_whitespace().nth(1)?.parse::<i32>().ok())
            == Some(yo)
    }

    /// R1-H19: el huerfano es de un node que todavia corre (de otro worker).
    fn de_un_node_en_curso(pid: i32, grupo: i32, mi_grupo: i32, yo: i32) -> bool {
        if grupo != pid && grupo != mi_grupo && lider_vivo(grupo, yo) {
            return true;
        }
        std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|d| super::cwd_en_curso(&d))
    }

    /// Hijos directos de este proceso: `/proc/self/task/*/children`
    /// (CONFIG_PROC_CHILDREN). Si no existe, recorre /proc buscando ppid.
    fn hijos() -> Vec<i32> {
        let mut v = Vec::new();
        let mut alguno = false;
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for t in tasks.flatten() {
                if let Ok(s) = std::fs::read_to_string(t.path().join("children")) {
                    alguno = true;
                    v.extend(s.split_whitespace().filter_map(|p| p.parse::<i32>().ok()));
                }
            }
        }
        if alguno {
            return v;
        }
        let yo = std::process::id() as i32;
        let Ok(procs) = std::fs::read_dir("/proc") else {
            return v;
        };
        for p in procs.flatten() {
            let Some(pid) = p.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            let ppid = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|st| st[st.rfind(')')? + 1..].split_whitespace().nth(1)?.parse::<i32>().ok());
            if ppid == Some(yo) {
                v.push(pid);
            }
        }
        v
    }

    pub(super) fn cosechar() -> usize {
        // SAFETY: getsid/getpgrp no toman punteros.
        let (mi_sesion, mi_grupo) = unsafe { (libc::getsid(0), libc::getpgrp()) };
        let mut n = 0;
        // Matar a un huerfano puede dejar huerfanos a sus hijos: se repite hasta
        // que no aparezca ninguno (con tope).
        for _ in 0..8 {
            let mut esta_vuelta = 0;
            for pid in hijos() {
                let Some((grupo, sesion)) = grupo_y_sesion(pid) else {
                    continue;
                };
                if !es_huerfano(pid, grupo, sesion, mi_grupo, mi_sesion) {
                    continue;
                }
                if de_un_node_en_curso(pid, grupo, mi_grupo, std::process::id() as i32) {
                    continue;
                }
                // SAFETY: pid es hijo nuestro sin cosechar: no se reusa. El grupo
                // (si es lider) tambien es ajeno.
                unsafe {
                    if grupo == pid {
                        libc::kill(-grupo, libc::SIGKILL);
                    }
                    libc::kill(pid, libc::SIGKILL);
                    if libc::waitpid(pid, std::ptr::null_mut(), 0) == pid {
                        esta_vuelta += 1;
                    }
                }
            }
            n += esta_vuelta;
            if esta_vuelta == 0 {
                break;
            }
        }
        n
    }

    pub(super) fn es_huerfano(pid: i32, grupo: i32, sesion: i32, mi_grupo: i32, mi_sesion: i32) -> bool {
        sesion != mi_sesion || (grupo != pid && grupo != mi_grupo)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::linux::es_huerfano;

    #[test]
    fn regla_de_huerfano() {
        let (g, s) = (100, 50);
        // node del runner o el hijo que minifica: lider de su grupo, nuestra sesion.
        assert!(!es_huerfano(7, 7, s, g, s));
        // node --check: nuestro grupo.
        assert!(!es_huerfano(8, g, s, g, s));
        // detached (setsid): otra sesion.
        assert!(es_huerfano(9, 9, 9, g, s));
        // resto del grupo de un node muerto.
        assert!(es_huerfano(10, 7, s, g, s));
    }

    /// R1-H17: el subreaper es de todo el proceso. Los tests que lo activan
    /// corren en un proceso de tests aparte (este binario filtrado al test, con
    /// `DIFUZZ_TEST_SUBREAPER`): asi no queda activo para los demas tests.
    /// Devuelve `true` si ya estamos en el proceso aparte (hay que correr el test).
    fn en_proceso_aparte(test: &str) -> bool {
        if std::env::var_os("DIFUZZ_TEST_SUBREAPER").is_some() {
            return true;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--test-threads", "1", "-q"])
            .env("DIFUZZ_TEST_SUBREAPER", "1")
            .output()
            .unwrap();
        let txt = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && txt.contains("1 passed"), "{test} en proceso aparte:\n{txt}");
        false
    }

    fn hay_node() -> bool {
        let ok = std::process::Command::new("node").arg("--version").output().is_ok();
        if !ok {
            eprintln!("sin node en PATH: test omitido");
        }
        ok
    }

    /// Un hijo `detached` del programa no sobrevive a la ejecucion, y con
    /// `stdio: 'inherit'` no hace esperar READ_GRACE.
    #[test]
    fn hijo_detached_no_sobrevive() {
        use crate::runner::{Preload, Runner, Status};
        use std::time::{Duration, Instant};
        if !hay_node() || !en_proceso_aparte("huerfanos::tests::hijo_detached_no_sobrevive") {
            return;
        }
        assert!(super::activar());
        let preload = Preload::write_tagged("subreaper").unwrap();
        // Id propio: con el 9988 compartia cwd y ctl con `stubs_en_node` y, en
        // paralelo, uno borraba el cwd del otro (spawn node: ENOENT).
        let r = Runner::new(&preload, 9984, Duration::from_secs(20));
        let vivo = |pid: i32| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.starts_with(b"sleep\x0030"))
        };
        // Sin cota absoluta de tiempo (fallaba bajo carga): lo que se mide es
        // que no se espere READ_GRACE (1 s) de mas sobre un programa trivial.
        let base = (0..2)
            .map(|_| {
                let t0 = Instant::now();
                assert_eq!(r.run("console.log(1);\n").status, Status::Exited(0));
                t0.elapsed()
            })
            .max()
            .unwrap();
        for stdio in ["ignore", "inherit"] {
            let js = format!(
                "const c = require('child_process').spawn('sleep', ['30'], {{ detached: true, stdio: '{stdio}' }});\nc.unref();\nconsole.log(c.pid);\n"
            );
            let t0 = Instant::now();
            let e = r.run(&js);
            assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
            let pid: i32 = e.stdout.trim().parse().unwrap();
            assert!(!vivo(pid), "stdio {stdio}: el sleep {pid} sigue vivo");
            assert!(t0.elapsed() < base + Duration::from_millis(800), "stdio {stdio}: tardo {:?} (base {base:?})", t0.elapsed());
        }
    }

    /// R1-H19: dos workers comparten el subreaper. El programa del worker B deja
    /// un nieto huerfano (el `sh` sale; el subshell pasa a difuzz) que necesita
    /// mientras sigue corriendo. La cosecha del worker A (simulada en este hilo,
    /// como la haria su runner tras cada ejecucion) no lo puede matar: si lo
    /// mata, la salida de B cambia ("nieto muerto") y seria una divergencia o
    /// un no determinismo falso. MEDIDO: sin el arreglo, este test falla.
    #[test]
    fn no_mata_huerfanos_de_un_node_en_curso() {
        use crate::runner::{Preload, Runner, Status};
        use std::time::Duration;
        if !hay_node() || !en_proceso_aparte("huerfanos::tests::no_mata_huerfanos_de_un_node_en_curso") {
            return;
        }
        assert!(super::activar());
        let preload = Preload::write_tagged("subreaper-h19").unwrap();
        let rb = Runner::new(&preload, 9983, Duration::from_secs(20));
        // Un nieto en el grupo de node y otro que hace setsid (otra sesion).
        let js = "const { spawn } = require('child_process');\nconst fs = require('fs');\nspawn('sh', ['-c', '(sleep 0.5; echo x > grupo) &'], { stdio: 'ignore' });\nspawn('sh', ['-c', 'setsid sh -c \\'sleep 0.5; echo x > sesion\\' &'], { stdio: 'ignore' });\nsetTimeout(() => console.log(fs.existsSync('grupo'), fs.existsSync('sesion')), 1500);\n";
        let fin = std::sync::atomic::AtomicBool::new(false);
        let e = std::thread::scope(|s| {
            let b = s.spawn(|| {
                let e = rb.run(js);
                fin.store(true, std::sync::atomic::Ordering::SeqCst);
                e
            });
            // Worker A: cosecha cada 20 ms mientras B corre. Puede cosechar
            // zombis (un nieto que ya termino: MEDIDO, estado Z), no vivos.
            while !fin.load(std::sync::atomic::Ordering::SeqCst) {
                super::cosechar();
                std::thread::sleep(Duration::from_millis(20));
            }
            b.join().unwrap()
        });
        assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
        assert_eq!(e.stdout, "true true\n", "la cosecha de otro worker mato nietos de un node en curso");
    }
}
