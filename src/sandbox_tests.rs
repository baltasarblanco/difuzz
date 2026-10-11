//! n-sandbox: `--sandbox` y `--sandbox-red` con node de verdad.
//!
//! - El runner: con el sandbox, leer fuera del directorio de trabajo (un senuelo propio,
//!   `/etc/passwd`, una ruta inexistente de `~/.ssh`), escribir afuera, `child_process`,
//!   workers y `process.binding` fallan y la ejecucion queda marcada (`toco_sandbox`); lo de
//!   adentro del directorio de trabajo anda. Sin el sandbox, lo de siempre (y nunca marca).
//! - El caso entero (`process_case`, como e2e_tests): 7.9, si el original y el minificado tocan
//!   el sandbox, el caso se compara igual (con una nota en meta.txt): con un "bug" puesto a
//!   mano va a div/, como sin el flag; sin bug, a iguales.
//! - Las senales (2b): con el sandbox, `process.kill` a otro proceso da EPERM (y marca); a si
//!   mismo, a 0 y a su grupo anda (7.2: tambien con el `process.pid` ficticio, sin marcar).
//! - La red (`--sandbox-red`, solo Linux con user namespaces): sin salida (ENETUNREACH), el
//!   loopback propio anda, los sockets unix del sistema quedan tapados; con `--sandbox`
//!   solo, un socket unix de afuera se alcanza (limite documentado del modelo de node).
//! - `node:sqlite` (7.10): con el sandbox va apagado; una base senuelo fuera del directorio
//!   de trabajo no se crea.
//!
//! Nada lee secretos: `/etc/passwd` y `~/.ssh` solo se tocan CON el sandbox (se verifica el
//! bloqueo, no el contenido), y en `~/.ssh` con un nombre que no existe.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::backend::identidad::Identidad;
use crate::backend::{Backend, Minifier};
use crate::minify::{Config, Outcome, SourceKind};
use crate::runner::{Preload, Runner, Sandbox, Status};

/// Lo de `Identidad` con `de` cambiado por `a` en el minificado (un "bug" a mano).
struct Cambia {
    de: String,
    a: String,
    kind: SourceKind,
}

impl Backend for Cambia {
    fn prepare(&self, src: &str) -> Outcome {
        match (Identidad { kind: self.kind }).prepare(src) {
            Outcome::Ok { original, minified, .. } => {
                assert!(minified.contains(&self.de), "Cambia: {:?} no esta en {minified:?}", self.de);
                let minified = minified.replacen(&self.de, &self.a, 1);
                let (static_flags, dead_writes, analisis) =
                    crate::backend::swc::checks_on_text(src, &original, &minified, self.kind, Minifier::Identidad);
                Outcome::Ok { original, minified, static_flags, dead_writes, analisis }
            }
            o => o,
        }
    }
}

/// Directorio propio del test (fuera del directorio de trabajo del runner) con un senuelo.
fn base(tag: &str) -> (PathBuf, PathBuf) {
    let b = std::env::temp_dir().join(format!("difuzz-test-{}-sandbox-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&b);
    std::fs::create_dir_all(&b).unwrap();
    let senuelo = b.join("senuelo.txt");
    std::fs::write(&senuelo, "senuelo de difuzz: no es un secreto\n").unwrap();
    (b, senuelo)
}

fn js_str(p: &Path) -> String {
    format!("{:?}", p.to_str().unwrap())
}

/// Corre `cuerpo` adentro de un try (imprime `ok` o `err <code>`) y devuelve el stdout,
/// si toco el sandbox y el estado.
fn intento(r: &Runner, cuerpo: &str) -> (String, bool, Status) {
    let js = format!("try {{ {cuerpo}; console.log('ok'); }} catch (e) {{ console.log('err', e.code); }}\n");
    let e = r.run(&js);
    (e.stdout.trim().to_string(), e.toco_sandbox(), e.status)
}

#[test]
fn sandbox_bloquea_y_marca() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("runner");
    let preload = Preload::write_tagged("sandbox").unwrap();
    let t = Duration::from_secs(20);
    let con = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(Sandbox::Permisos);
    let sin = Runner::new(&preload, crate::test_util::id_runner(), t);
    let fuera = b.join("escrito-desde-node.txt");
    let ssh = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into())).join(".ssh/difuzz-senuelo-que-no-existe");
    let fs = "require('fs')";
    let negados = [
        format!("{fs}.readFileSync({})", js_str(&senuelo)),
        format!("{fs}.readFileSync('/etc/passwd')"),
        format!("{fs}.readFileSync({})", js_str(&ssh)),
        format!("{fs}.writeFileSync({}, 'x')", js_str(&fuera)),
        "require('child_process').execSync('true')".into(),
        "new (require('worker_threads').Worker)('1', { eval: true })".into(),
        "process.binding('fs')".into(),
    ];
    for c in &negados {
        let (out, toco, st) = intento(&con, c);
        assert!(out.starts_with("err ERR_ACCESS_DENIED"), "{c}: {out}");
        assert!(toco, "{c}: no marco el sandbox");
        assert_eq!(st, Status::Exited(0), "{c}");
    }
    assert!(!fuera.exists(), "el sandbox dejo escribir fuera del directorio de trabajo");
    // Adentro del directorio de trabajo (el cwd del programa) se lee y se escribe.
    let (out, toco, _) = intento(&con, &format!("{fs}.writeFileSync('a.txt', 'x'); if ({fs}.readFileSync('a.txt', 'utf8') !== 'x') throw 1"));
    assert_eq!((out.as_str(), toco), ("ok", false));
    // Un symlink del directorio de trabajo hacia el senuelo no sirve para salir.
    let (out, toco, _) = intento(&con, &format!("{fs}.symlinkSync({}, 'l'); {fs}.readFileSync('l')", js_str(&senuelo)));
    assert!(out.starts_with("err ERR_ACCESS_DENIED") && toco, "symlink: {out}");
    // Sin atrapar: lanza (exit 70) y tambien marca (el preload lo ve en onThrow).
    let e = con.run(&format!("{fs}.readFileSync({});\n", js_str(&senuelo)));
    assert!(e.threw() && e.toco_sandbox(), "{:?} {}", e.status, e.ctl);
    // Asincronico, atrapado por el programa: lo ve la salida.
    let e = con.run(&format!("{fs}.readFile({}, (err) => console.log(err && err.code));\n", js_str(&senuelo)));
    assert!(e.stdout.contains("ERR_ACCESS_DENIED") && e.toco_sandbox(), "{}", e.stdout);

    // Sin el sandbox, lo de siempre: el senuelo se lee, la ruta de ~/.ssh no existe (ENOENT,
    // no se leyo nada) y nada marca, ni un programa que imprime el texto de la marca.
    let (out, toco, _) = intento(&sin, &format!("{fs}.readFileSync({})", js_str(&senuelo)));
    assert_eq!((out.as_str(), toco), ("ok", false));
    let (out, toco, _) = intento(&sin, &format!("{fs}.readFileSync({})", js_str(&ssh)));
    assert_eq!((out.as_str(), toco), ("err ENOENT", false));
    let e = sin.run("console.log('ERR_ACCESS_DENIED');\n");
    assert!(!e.toco_sandbox());
    // Una linea `sandbox` en el canal (la escribiria un programa que adivina su ruta) no
    // cambia nada sin el sandbox: main pregunta al runner.
    let mut e = sin.run("console.log(1);\n");
    e.ctl.push_str("sandbox\n");
    assert!(e.toco_sandbox() && !sin.toco_sandbox(&e) && con.toco_sandbox(&e));
    drop((con, sin));
    let _ = std::fs::remove_dir_all(&b);
}

/// El sensor de violaciones sigue andando con el sandbox (`--allow-inspector`), y un
/// permiso negado que el programa atrapa en silencio lo ve el sensor.
#[test]
fn sandbox_con_sensor() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("sensor");
    let preload = Preload::write_tagged("sandbox-sensor").unwrap();
    let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_sandbox(Sandbox::Permisos);
    let e = r.run("try { x; } catch (e) { console.log('atrapada'); }\nlet x = 1;\n");
    assert!(e.flags.iter().any(|f| f == "tdz_atrapada"), "{:?} {}", e.flags, e.ctl);
    assert!(!e.toco_sandbox());
    let e = r.run(&format!("try {{ require('fs').readFileSync({}); }} catch {{}}\nconsole.log(1);\n", js_str(&senuelo)));
    assert_eq!(e.stdout.trim(), "1");
    assert!(e.toco_sandbox(), "atrapado en silencio: {}", e.ctl);
    // b7-1b (revision 7.1, H5): con `--allow-inspector` el programa no abre un puerto del
    // inspector: `open` lo niega el preload (y marca), `_debugProcess` y SIGUSR1 los corta
    // `--disable-sigusr1`. Lo mismo en una corrida con cobertura.
    for m in ["inspector", "inspector/promises"] {
        let (out, toco, _) = intento(&r, &format!("require('{m}').open(0, '127.0.0.1')"));
        assert!(out.starts_with("err ERR_ACCESS_DENIED") && toco, "{m}: {out}");
    }
    for c in ["process._debugProcess(process.pid)", "process.kill(process.pid, 'SIGUSR1')"] {
        // Con el sandbox, `_debugProcess` ademas lanza (lo niega el filtro de senales).
        let js = format!("try {{ {c}; }} catch {{}}\nsetTimeout(() => console.log(String(require('inspector').url())), 300);\n");
        for e in [r.run(&js), r.run_con_cobertura(&js, &[], &[])] {
            assert_eq!(e.stdout.trim(), "undefined", "{c}: {:?} {}", e.status, e.ctl);
        }
    }
    drop(r);
    let _ = std::fs::remove_dir_all(&b);
}

/// Un caso entero: con `--sandbox`, el original que toca un permiso (y su minificado
/// tambien) se compara igual (7.9, revision fe0cd86 MENOR 3; antes iba a descartes con causa
/// `sandbox` y se perdia el bug): con un "bug" despues del permiso negado y atrapado, div/
/// con la nota de A-y-B en meta.txt, como sin el flag; sin bug (el mensaje entero del error
/// impreso, o el mismo error sin atrapar), iguales. Y un minificado que toca el sandbox
/// cuando el original no (el bug cambia la ruta) se compara: div/. 2c: y al reves, un original que lo toca cuando el minificado no (el
/// bug hace que B no lea), tambien: div/, con la nota en meta.txt.
#[test]
fn sandbox_destino_del_caso() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("caso");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    let leer = format!("var t = require('fs').readFileSync({}, 'utf8');\nconsole.log(\"bug-no\", t.length);\n", js_str(&senuelo));
    let hijo = "require('child_process').execSync('true');\nconsole.log(\"bug-no\");\n".to_string();
    let escribir = "require('fs').writeFileSync('dentro.txt', 'x');\nconsole.log(41);\n".to_string();
    // revision-n2/sb3/u01-solo-a-toca: el original lee (negado, atrapado e impreso); con el
    // "bug", B no entra a la rama.
    let solo_a = format!(
        "var m = \"bug-no\";\nif (m === \"bug-no\") {{ try {{ require('fs').readFileSync({}); }} catch (e) {{ console.log(\"negado\", e.code); }} }}\nconsole.log(m);\n",
        js_str(&senuelo)
    );
    let vivo = "var vivo; try { process.kill(process.pid, 0); vivo = true; } catch (e) { vivo = e.code; }\nconsole.log(vivo, \"bug-no\");\n".to_string();
    let matarse = "var r; try { process.kill(process.pid, \"SIGTERM\"); r = \"ok\"; } catch (e) { r = e.code; }\nconsole.log(r, \"bug-no\");\n".to_string();
    // revision/a3-2c/corpus/c01-ayb-bug-real: lee negado y lo atrapa en silencio; despues, el
    // "bug" (ajeno al sandbox). Antes de 7.9 iba a descartes.
    let ayb_bug = format!("try {{ require('fs').readFileSync({}); }} catch (e) {{}}\nconsole.log(\"bug-no\");\n", js_str(&senuelo));
    // 7.9: sin bug (el "minificado" cambia `1` por `0+1`), A y B leen negado e imprimen el error
    // entero (codigo, mensaje con la ruta, permiso y recurso): iguales.
    let sin_bug = format!(
        "var n = 1;\ntry {{ require('fs').readFileSync({}); }} catch (e) {{ console.log(e.code, e.message, e.permission, e.resource); }}\nconsole.log(n);\n",
        js_str(&senuelo)
    );
    // (caso, programa, (de, a) del "bug", flags, destino esperado)
    type Caso<'a> = (&'a str, String, (&'a str, String), &'a [&'a str], &'a str);
    let casos: Vec<Caso> = vec![
        // Con el sandbox, A y B lanzan el mismo ERR_ACCESS_DENIED antes de llegar al "bug"
        // (7.9: se comparan; antes, descarte): iguales. Sin el flag, el bug se ve: div/.
        ("leer-con", leer.clone(), ("bug-no", "bug-si".into()), &["--sandbox"], "iguales"),
        ("leer-sin", leer, ("bug-no", "bug-si".into()), &[], "div"),
        ("hijo-con", hijo.clone(), ("bug-no", "bug-si".into()), &["--sandbox"], "iguales"),
        ("hijo-sin", hijo, ("bug-no", "bug-si".into()), &[], "div"),
        ("ayb-bug", ayb_bug, ("bug-no", "bug-si".into()), &["--sandbox"], "div"),
        ("ambos-sin-bug", sin_bug, ("var n=1", "var n=0+1".into()), &["--sandbox"], "iguales"),
        // El original escribe adentro; el minificado (el bug) escribe afuera: B-solo.
        ("solo-b", escribir, ("dentro.txt", b.join("fuera.txt").to_str().unwrap().to_string()), &["--sandbox"], "div"),
        ("solo-a", solo_a, ("bug-no", "bug-si".into()), &["--sandbox"], "div"),
        // 7.2 (revision 10-oct, c02-ayb-kill-cero): "esta vivo?" con el pid ficticio no toca
        // el sandbox: el bug se compara (con fe0cd86 iba a descartes `sandbox`).
        ("kill-propio", vivo.clone(), ("bug-no", "bug-si".into()), &["--sandbox"], "div"),
        ("kill-propio-sin", vivo, ("bug-no", "bug-si".into()), &[], "div"),
        // 7.2 (k17-pid-falso): matarse muere por la senal, como en node: descarte `senal`.
        ("matarse", matarse.clone(), ("bug-no", "bug-si".into()), &["--sandbox"], "descarte:senal"),
        ("matarse-sin", matarse, ("bug-no", "bug-si".into()), &[], "descarte:senal"),
    ];
    let preload = Preload::write_tagged("sandbox-caso").unwrap();
    let known = crate::known::Known::load(&corpus, &out, 0, Minifier::Oxc);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    for (caso, js, (de, a), extra, esperado) in casos {
        std::fs::create_dir_all(corpus.join(caso)).unwrap();
        std::fs::write(corpus.join(caso).join("input.js"), &js).unwrap();
        let mut args: Vec<String> = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        let cli = crate::Cli::parse(args).unwrap();
        let celda = out.join(caso);
        let reporter = crate::oracle::Reporter::new(&celda, cli.cell(), crate::runner::PRELOAD);
        let runner = crate::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
        let cfg = Config::from_backend(Box::new(Cambia { de: de.into(), a, kind: SourceKind::Script }));
        let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let mut st = crate::Stats::default();
        crate::process_case(&ctx, &runner, &cfg, None, &cli, &mut st, &corpus.join(caso).join("input.js"));
        drop(runner);
        let tsv = std::fs::read_to_string(celda.join("descartes.tsv")).unwrap_or_default();
        let causa = tsv.lines().filter(|l| !l.starts_with('#')).find_map(|l| l.split('\t').nth(1).map(|c| format!("descarte:{c}")));
        let donde = if celda.join("div").join(caso).exists() {
            "div".to_string()
        } else if celda.join("known").join(caso).exists() {
            "known".to_string()
        } else {
            causa.clone().unwrap_or_else(|| "iguales".into())
        };
        assert_eq!(donde, esperado, "{caso}: {} | {tsv}", st.line());
        // 7.9: nada va a descartes por el sandbox (B corrio en todos).
        let ambos = ["leer-con", "hijo-con", "ayb-bug", "ambos-sin-bug"].contains(&caso);
        assert_eq!(st.a_sandbox, 0, "{caso}");
        assert_eq!(st.solo_a_sandbox, usize::from(caso == "solo-a"), "{caso}");
        assert_eq!(st.ambos_sandbox, usize::from(ambos), "{caso}");
        if esperado == "div" {
            assert!(causa.is_none(), "{caso}: div/ y ademas descarte {causa:?}");
            let meta = std::fs::read_to_string(celda.join("div").join(caso).join("meta.txt")).unwrap();
            assert_eq!(meta.matches("solo el original toco el sandbox").count(), usize::from(caso == "solo-a"), "{caso}: {meta}");
            assert_eq!(meta.matches("el original y el minificado tocaron el sandbox").count(), usize::from(ambos), "{caso}: {meta}");
        }
        if esperado == "iguales" {
            assert_eq!((st.same, st.divergences, st.known), (1, 0, 0), "{caso}: {}", st.line());
        }
    }
    drop(preload);
    let _ = std::fs::remove_dir_all(&b);
}

/// 2b: con `--sandbox` y con `--sandbox-red` (7.5: sin user namespaces falla, salvo
/// `DIFUZZ_SIN_USERNS=1`, que saltea solo `--sandbox-red`), node no le manda
/// senales a otro proceso: EPERM, y la ejecucion queda marcada. El senuelo (un `sleep`
/// propio) recibe SIGTERM solo si el filtro falla; a este test (el padre de node) y a todo el
/// usuario (`-1`) se les manda la senal 0, que no hace nada aunque llegue. A si mismo (el pid
/// real, de `process.report`), a 0 y a su grupo (pgid = pid de node) anda. 7.2: `process.pid`
/// (ficticio, M20) tambien es si mismo, sin marcar (antes daba EPERM y el caso iba a
/// descartes), y `process.ppid` da ESRCH sin llamar al sistema (nunca el padre real), con o
/// sin el sandbox. Sin el sandbox, lo de siempre.
#[test]
fn sandbox_senales() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let preload = Preload::write_tagged("sandbox-senales").unwrap();
    let t = Duration::from_secs(20);
    let mut modos = vec![Sandbox::Permisos];
    // 7.5: sin user namespaces falla (salvo DIFUZZ_SIN_USERNS=1, test_util.rs).
    if crate::test_util::requiere_userns(crate::runner::probar_sandbox(Sandbox::PermisosYRed)) {
        modos.push(Sandbox::PermisosYRed);
    }
    let mut senuelo = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let (pid, padre) = (senuelo.id(), std::process::id());
    let yo = "process.report.getReport().header.processId";
    for modo in modos {
        let r = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(modo);
        let afuera = [
            (pid.to_string(), "'SIGTERM'"),
            (padre.to_string(), "0"),
            ("-1".into(), "0"),
        ];
        for (destino, senal) in afuera {
            let (out, toco, st) = intento(&r, &format!("process.kill({destino}, {senal})"));
            assert_eq!((out.as_str(), toco, st), ("err EPERM", true, Status::Exited(0)), "{modo:?} {destino}");
        }
        // Sin atrapar: lanza y marca (el mensaje `kill EPERM` en la linea `exc`).
        let e = r.run(&format!("process.kill({pid}, 'SIGTERM');\n"));
        assert!(e.threw() && e.toco_sandbox(), "{modo:?}: {:?} {}", e.status, e.ctl);
        for destino in [yo.to_string(), "0".into(), format!("-{yo}"), "process.pid".into(), "-process.pid".into()] {
            let (out, toco, _) = intento(&r, &format!("process.kill({destino}, 0)"));
            assert_eq!((out.as_str(), toco), ("ok", false), "{modo:?} {destino}");
        }
        // 7.2: el ppid ficticio da ESRCH, sin llamar al sistema (no marca).
        for destino in ["process.ppid", "-process.ppid"] {
            let (out, toco, st) = intento(&r, &format!("process.kill({destino}, 0)"));
            assert_eq!((out.as_str(), toco, st), ("err ESRCH", false, Status::Exited(0)), "{modo:?} {destino}");
        }
        // 7.2: matarse con el pid ficticio anda como en node (muere por la senal) y no marca.
        let e = r.run("process.kill(process.pid, 'SIGTERM');\nconsole.log('sigue');\n");
        assert_eq!((e.status.clone(), e.stdout.as_str(), e.toco_sandbox()), (Status::Signal, "", false), "{modo:?}: {}", e.stderr);
        let e = r.run("process.on('SIGTERM', () => console.log('recibida'));\nprocess.kill(process.pid, 'SIGTERM');\nsetTimeout(() => {}, 50);\n");
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("recibida", false), "{modo:?}: {}", e.stderr);
        // Y la senal llega: un handler propio la recibe (el timer mantiene vivo el loop hasta
        // que llega; sin el, node termina antes tambien sin el sandbox: MEDIDO).
        let e = r.run(&format!("process.on('SIGUSR2', () => console.log('recibida'));\nprocess.kill({yo}, 'SIGUSR2');\nsetTimeout(() => {{}}, 50);\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("recibida", false), "{modo:?}: {}", e.stderr);
        assert!(senuelo.try_wait().unwrap().is_none(), "{modo:?}: el senuelo recibio la senal");
    }
    let sin = Runner::new(&preload, crate::test_util::id_runner(), t);
    let (out, toco, _) = intento(&sin, &format!("process.kill({pid}, 0)"));
    assert_eq!((out.as_str(), toco), ("ok", false));
    // 7.2: sin el sandbox, lo mismo que con el: el pid ficticio es node y el ppid, ESRCH.
    let (out, toco, _) = intento(&sin, "process.kill(process.pid, 0)");
    assert_eq!((out.as_str(), toco), ("ok", false));
    let (out, toco, _) = intento(&sin, "process.kill(process.ppid, 0)");
    assert_eq!((out.as_str(), toco), ("err ESRCH", false));
    assert!(senuelo.try_wait().unwrap().is_none(), "sin sandbox: el senuelo recibio la senal");
    let _ = senuelo.kill();
    let _ = senuelo.wait();
}

/// 7.10: `node:sqlite` se saltea el modelo de permisos (MEDIDO, b7-9: crea una base fuera de lo
/// permitido). Con el sandbox va apagado (`--no-experimental-sqlite`): `require` da
/// ERR_UNKNOWN_BUILTIN_MODULE, el senuelo (una base en el directorio del test, fuera del de
/// trabajo) no se crea y, si el mensaje sale (sin atrapar, o impreso), la ejecucion queda
/// marcada. El codigo solo no marca: tambien lo da un `node:` que no existe. Sin el sandbox,
/// lo de siempre: el modulo esta y nada marca.
#[test]
fn sandbox_sqlite_apagado() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, _) = base("sqlite");
    let db = b.join("senuelo.db");
    let preload = Preload::write_tagged("sandbox-sqlite").unwrap();
    let t = Duration::from_secs(20);
    let mut modos = vec![Sandbox::Permisos];
    match crate::runner::probar_sandbox(Sandbox::PermisosYRed) {
        Ok(()) => modos.push(Sandbox::PermisosYRed),
        Err(e) => eprintln!("DIFUZZ_SALTEADO_SIN_USERNS: sandbox_sqlite_apagado con la red: {e}"),
    }
    let crear = format!("new (require('node:sqlite').DatabaseSync)({}).exec('CREATE TABLE t (x)')", js_str(&db));
    for modo in modos {
        let r = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(modo);
        let (out, _, st) = intento(&r, &crear);
        assert_eq!((out.as_str(), st), ("err ERR_UNKNOWN_BUILTIN_MODULE", Status::Exited(0)), "{modo:?}");
        assert!(!db.exists(), "{modo:?}: el sandbox dejo crear la base sqlite");
        // Impreso: marca por el mensaje.
        let e = r.run("try { require('node:sqlite'); } catch (e) { console.log(e.message); }\n");
        assert!(e.toco_sandbox(), "{modo:?}: {}", e.stdout);
        // Sin atrapar (el mensaje en la linea `exc` del canal) y por `import`, igual.
        let e = r.run(&format!("{crear};\n"));
        assert!(e.threw() && e.toco_sandbox(), "{modo:?}: {:?} {}", e.status, e.ctl);
        let e = r.run("import('node:sqlite').catch((e) => console.log(e.message));\n");
        assert!(e.toco_sandbox(), "{modo:?}: {}", e.stdout);
        assert!(!db.exists(), "{modo:?}");
        // Otro `node:` que no existe da el mismo codigo y no marca.
        let (out, toco, _) = intento(&r, "require('node:difuzz-no-existe')");
        assert_eq!((out.as_str(), toco), ("err ERR_UNKNOWN_BUILTIN_MODULE", false), "{modo:?}");
    }
    // Sin el sandbox: el modulo esta (la base va al directorio de trabajo del runner) y no marca.
    let sin = Runner::new(&preload, crate::test_util::id_runner(), t);
    let (out, toco, _) = intento(&sin, "new (require('node:sqlite').DatabaseSync)('adentro.db').exec('CREATE TABLE t (x)')");
    assert_eq!((out.as_str(), toco), ("ok", false));
    drop(sin);
    let _ = std::fs::remove_dir_all(&b);
}

/// La celda y la cache: `--sandbox` y `--sandbox-red` cambian el nombre de la celda; sin
/// el flag, el de siempre.
#[test]
fn sandbox_en_la_celda() {
    let cli = |extra: &[&str]| crate::Cli::parse(["c"].iter().chain(extra).map(|s| s.to_string()).collect()).unwrap();
    assert_eq!(cli(&[]).cell(), "oxc+carril_a+tal_cual");
    assert_eq!(cli(&["--sandbox"]).cell(), "oxc+carril_a+tal_cual+sandbox");
    assert_eq!(cli(&["--sandbox-red"]).cell(), "oxc+carril_a+tal_cual+sandbox_red");
    assert_eq!(cli(&["--sandbox-red", "--sandbox"]).sandbox, Sandbox::PermisosYRed);
    assert_eq!(cli(&["--stubs", "--sandbox"]).cell(), "oxc+carril_a+tal_cual+stubs+sandbox");
}

/// `--sandbox-red`: necesita que esta maquina arme los namespaces (si no, falla; con
/// `DIFUZZ_SIN_USERNS=1` se saltea con una linea en stderr, 7.5; `probar_sandbox` es lo que
/// corta la corrida en ese caso).
#[test]
fn sandbox_red() {
    if !crate::test_util::requiere_node() {
        return;
    }
    // 7.5: sin user namespaces falla (salvo DIFUZZ_SIN_USERNS=1, test_util.rs).
    if !crate::test_util::requiere_userns(crate::runner::probar_sandbox(Sandbox::PermisosYRed)) {
        return;
    }
    let (b, _) = base("red");
    // Un socket unix "del sistema" (fuera de /run): uno propio, escuchando.
    let sock = b.join("servicio.sock");
    let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    l.set_nonblocking(true).unwrap();
    let preload = Preload::write_tagged("sandbox-red").unwrap();
    let t = Duration::from_secs(20);
    let red = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(Sandbox::PermisosYRed);
    let solo = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(Sandbox::Permisos);
    let conectar = |destino: &str| {
        format!(
            "const s = require('net').connect({destino});\
             s.on('connect', () => {{ console.log('conecto'); s.destroy(); }});\
             s.on('error', (e) => console.log('err', e.code));\n"
        )
    };
    // Afuera (192.0.2.1 es TEST-NET-1, documentacion): sin ruta en el namespace, no sale nada.
    let e = red.run(&conectar("9, '192.0.2.1'"));
    assert_eq!(e.stdout.trim(), "err ENETUNREACH");
    assert!(e.toco_sandbox());
    // El loopback propio anda: un servidor y un cliente del mismo programa.
    let e = red.run(
        "const net = require('net');\
         const srv = net.createServer((c) => c.end('hola')).listen(0, '127.0.0.1', () => {\
           const c = net.connect(srv.address().port, '127.0.0.1');\
           c.on('data', (d) => { console.log(String(d)); srv.close(); });\
         });\n",
    );
    assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("hola", false), "{}", e.stderr);
    // DNS: sin /run no hay resolvedor; un nombre que no esta en /etc/hosts no resuelve (y
    // no sale nada: no hay red). Marca.
    let e = red.run("require('dns').lookup('difuzz-no-existe.invalid', (err) => console.log('dns', err && err.code));\n");
    assert!(e.stdout.starts_with("dns E") && e.toco_sandbox(), "{}", e.stdout);
    // El socket unix: con --sandbox solo se alcanza (limite de node); con la red, tapado.
    let e = solo.run(&conectar(&js_str(&sock)));
    assert_eq!(e.stdout.trim(), "conecto");
    let e = red.run(&conectar(&js_str(&sock)));
    assert!(e.stdout.trim().starts_with("err "), "{}", e.stdout);
    // /run tapado: el socket de systemd-resolved, si esta en esta maquina, se alcanza con
    // --sandbox solo (MEDIDO: por ahi resuelve DNS) y no con la red. Solo se conecta, no
    // se le manda nada.
    let resolved = Path::new("/run/systemd/resolve/io.systemd.Resolve");
    if resolved.exists() {
        let e = solo.run(&conectar(&js_str(resolved)));
        assert_eq!(e.stdout.trim(), "conecto");
        let e = red.run(&conectar(&js_str(resolved)));
        assert!(e.stdout.trim().starts_with("err "), "{}", e.stdout);
    }
    // El mismo uid adentro, y el timeout mata al node del namespace.
    // SAFETY: getuid no falla.
    let uid = unsafe { libc::getuid() };
    let e = red.run("console.log(process.getuid());\n");
    assert_eq!(e.stdout.trim(), uid.to_string());
    let corto = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_millis(800)).with_sandbox(Sandbox::PermisosYRed);
    let e = corto.run("while (true) {}\n");
    assert_eq!(e.status, Status::Timeout);
    drop((red, solo, corto, l));
    let _ = std::fs::remove_dir_all(&b);
}

/// 7.7 (MENOR 5): la nota de solo-A va una sola vez, y se saca sin tocar la del analisis.
#[test]
fn nota_solo_a_una_vez() {
    let n = crate::NOTA_SOLO_A;
    assert_eq!(crate::con_nota_solo_a(None), n);
    assert_eq!(crate::con_nota_solo_a(Some(n.to_string())), n);
    assert_eq!(crate::con_nota_solo_a(Some("analisis".into())), format!("analisis | {n}"));
    assert_eq!(crate::con_nota_solo_a(Some(format!("analisis | {n}"))), format!("analisis | {n}"));
    assert_eq!(crate::sin_nota_solo_a(Some(format!("analisis | {n}"))).as_deref(), Some("analisis"));
    assert_eq!(crate::sin_nota_solo_a(Some(n.to_string())), None);
    assert_eq!(crate::sin_nota_solo_a(Some("analisis".into())).as_deref(), Some("analisis"));
}

/// 7.7: a quien apunta una llamada de `setpriority` en el hijo (`Quien::Yo`: su pid).
#[derive(Clone, Copy, Debug)]
enum Quien {
    Cero,
    Yo,
    Otro(libc::id_t),
}

/// 7.7: `setpriority(which, quien, 19)` en el hijo de un `Command`, con o sin el filtro de
/// `encerrar_senales`, sin node: `pre_exec` instala el filtro y hace la llamada; si falla,
/// su errno vuelve como error del spawn y no hay exec. El hijo esta en un grupo propio
/// (`process_group(0)`, como node en `run_file`): `PRIO_PGRP` con 0 o con su pid es solo el.
/// 19 es la prioridad mas baja: sin privilegios solo se puede bajar.
fn prioridad_en_el_hijo(filtro: bool, which: libc::__priority_which_t, quien: Quien) -> Result<(), Option<i32>> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("true");
    cmd.process_group(0).stdin(std::process::Stdio::null());
    // SAFETY: entre fork y exec solo llamadas al sistema (encerrar_senales, getpid,
    // setpriority) sobre memoria de la pila.
    unsafe {
        cmd.pre_exec(move || {
            if filtro {
                crate::runner::encerrar_senales()?;
            }
            let who = match quien {
                Quien::Cero => 0,
                Quien::Yo => libc::getpid() as libc::id_t,
                Quien::Otro(w) => w,
            };
            if libc::setpriority(which, who, 19) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(mut c) => {
            let _ = c.wait();
            Ok(())
        }
        Err(e) => Err(e.raw_os_error()),
    }
}

/// 7.7 (revision fe0cd86, IMPORTANTE 1): el filtro deja `setpriority` solo sobre el propio
/// proceso y su grupo, y niega el resto con EPERM. Los destinos de afuera no existen (un pid
/// y un grupo mas alla de `pid_max`, un uid sin procesos): sin el filtro dan ESRCH (o
/// EINVAL, un `which` que no existe), asi que nada cambia aunque el filtro fallara; nunca
/// `PRIO_USER` con el uid propio.
#[test]
fn filtro_prioridad() {
    // Un uid sin procesos (`find_user` no lo encuentra: ESRCH). No es 0 ni el propio.
    const NADIE: libc::id_t = 3_999_999_999;
    let afuera = Quien::Otro(i32::MAX as libc::id_t);
    let (proceso, grupo, usuario) = (libc::PRIO_PROCESS, libc::PRIO_PGRP, libc::PRIO_USER);
    for (which, quien) in [(proceso, Quien::Cero), (proceso, Quien::Yo), (grupo, Quien::Cero), (grupo, Quien::Yo)] {
        assert_eq!(prioridad_en_el_hijo(true, which, quien), Ok(()), "{which} {quien:?}");
    }
    let negadas = [(proceso, afuera), (grupo, afuera), (usuario, Quien::Otro(NADIE)), (3, Quien::Cero)];
    for (which, quien) in negadas {
        assert_eq!(prioridad_en_el_hijo(true, which, quien), Err(Some(libc::EPERM)), "con el filtro: {which} {quien:?}");
    }
    // El control: sin el filtro, el kernel no daba EPERM (lo que cambia es el filtro).
    let sin = [libc::ESRCH, libc::ESRCH, libc::ESRCH, libc::EINVAL];
    for ((which, quien), errno) in negadas.into_iter().zip(sin) {
        assert_eq!(prioridad_en_el_hijo(false, which, quien), Err(Some(errno)), "sin el filtro: {which} {quien:?}");
    }
}

/// 7.7 (IMPORTANTE 1 y MENOR 4), con node: `os.setPriority` a otro proceso da EPERM y marca
/// (atrapado e impreso, atrapado en silencio con el sensor, sin atrapar); a si mismo anda
/// y no marca. `process._kill` (no lanza: devuelve -1) y `process._debugProcess` negados
/// marcan aunque el programa los atrape en silencio sin el sensor. El senuelo (un `sleep`
/// propio) no cambia de prioridad ni recibe nada. Sin el sandbox, `_kill` es el de node.
#[test]
fn sandbox_prioridad_y_senales_sin_lanzar() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let preload = Preload::write_tagged("sandbox-prioridad").unwrap();
    let t = Duration::from_secs(20);
    let mut modos = vec![Sandbox::Permisos];
    // 7.5: sin user namespaces falla (salvo DIFUZZ_SIN_USERNS=1, test_util.rs).
    if crate::test_util::requiere_userns(crate::runner::probar_sandbox(Sandbox::PermisosYRed)) {
        modos.push(Sandbox::PermisosYRed);
    }
    let mut senuelo = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let pid = senuelo.id();
    // SAFETY: getpriority no toca memoria. La prioridad del senuelo es >= 0 (sin privilegios).
    let prioridad = || unsafe { libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t) };
    let antes = prioridad();
    let yo = "process.report.getReport().header.processId";
    let os = "require('os')";
    for modo in modos {
        let r = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(modo);
        let mudo = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(modo).with_sensor(false);
        let (out, toco, st) = intento(&r, &format!("{os}.setPriority({pid}, 19)"));
        assert_eq!((out.as_str(), toco, st), ("err ERR_SYSTEM_ERROR", true, Status::Exited(0)), "{modo:?}");
        let e = r.run(&format!("try {{ {os}.setPriority({pid}, 19); }} catch {{}}\nconsole.log(1);\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("1", true), "{modo:?} en silencio, con el sensor: {}", e.ctl);
        let e = mudo.run(&format!("try {{ {os}.setPriority({pid}, 19); }} catch (e) {{ console.log(e.message); }}\n"));
        assert!(e.stdout.contains("uv_os_setpriority returned EPERM") && e.toco_sandbox(), "{modo:?}: {}", e.stdout);
        let e = mudo.run(&format!("{os}.setPriority({pid}, 19);\n"));
        assert!(e.threw() && e.toco_sandbox(), "{modo:?}: {:?} {}", e.status, e.ctl);
        // A si mismo (0 y su pid real) anda; leer la prioridad de otro, tambien (no se filtra).
        for destino in ["0".to_string(), yo.to_string()] {
            let (out, toco, _) = intento(&r, &format!("{os}.setPriority({destino}, 19)"));
            assert_eq!((out.as_str(), toco), ("ok", false), "{modo:?} {destino}");
        }
        let e = r.run(&format!("console.log({os}.getPriority({pid}) === {antes});\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("true", false), "{modo:?}: {}", e.stderr);
        // MENOR 4: sin el sensor, atrapado o sin lanzar.
        let e = mudo.run(&format!("console.log(process._kill({pid}, 15));\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("-1", true), "{modo:?}: {}", e.ctl);
        let e = mudo.run(&format!("try {{ process._debugProcess({pid}); }} catch {{}}\nconsole.log(1);\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("1", true), "{modo:?}: {}", e.ctl);
        // A si mismo, `_kill` anda (0) y no marca; el envoltorio tiene el `length` de node.
        let e = mudo.run(&format!("console.log(process._kill({yo}, 0), process._kill.length, process._kill.name);\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("0 0 _kill", false), "{modo:?}: {}", e.stderr);
        // Integracion 7.2 + 7.7: el pid ficticio va al real por `_kill` (no marca); a otro, si
        // marca (el EPERM del `_kill` nativo dentro del envoltorio de 7.2). `os.setPriority` y
        // `_debugProcess` con el pid ficticio no se traducen: el filtro los niega y marcan.
        let e = mudo.run("console.log(process._kill(process.pid, 0));\ntry { process.kill(process.pid, 0); console.log('vivo'); } catch (e) { console.log(e.code); }\n");
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("0\nvivo", false), "{modo:?}: {}", e.stderr);
        let e = mudo.run(&format!("try {{ process.kill({pid}, 0); }} catch {{}}\nconsole.log(1);\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("1", true), "{modo:?}: {}", e.ctl);
        let (out, toco, _) = intento(&r, &format!("{os}.setPriority(process.pid, 19)"));
        assert_eq!((out.as_str(), toco), ("err ERR_SYSTEM_ERROR", true), "{modo:?}");
        let e = mudo.run("try { process._debugProcess(process.pid); } catch (e) { console.log(e.code); }\n");
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("EPERM", true), "{modo:?}: {}", e.ctl);
        assert!(senuelo.try_wait().unwrap().is_none(), "{modo:?}: el senuelo recibio una senal");
        assert_eq!(prioridad(), antes, "{modo:?}: el senuelo cambio de prioridad");
    }
    // Sin el sandbox, el `_kill` de node (sin envolver) y nada marca.
    let sin = Runner::new(&preload, crate::test_util::id_runner(), t);
    let e = sin.run(&format!("console.log(process._kill({pid}, 0), /native code/.test(Function.prototype.toString.call(process._kill)));\n"));
    assert_eq!((e.stdout.trim(), sin.toco_sandbox(&e), e.toco_sandbox()), ("0 true", false, false), "{}", e.stderr);
    let _ = senuelo.kill();
    let _ = senuelo.wait();
}

/// 7.7 (MENOR 5 y 8): carril B, un modulo mudo cuyo original toca el sandbox y el minificado
/// no (`revision/a3-2c/corpus-b/m01-mudo-solo-a`): la pasada cruda (iguales: los dos mudos) y
/// el envoltorio de modulos son solo-A. El hallazgo (div/) lleva la nota UNA vez y el caso
/// cuenta dos pasadas solo-A.
#[test]
fn sandbox_solo_a_modulo() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("modulo");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    let caso = "m01-mudo-solo-a";
    let js = format!(
        "import fs from \"node:fs\";\nexport let r = \"x\";\nvar m = \"bug-no\";\nif (m === \"bug-no\") {{ try {{ fs.readFileSync({}); r = \"leyo\"; }} catch (e) {{ r = \"no\"; }} }}\n",
        js_str(&senuelo)
    );
    std::fs::create_dir_all(corpus.join(caso)).unwrap();
    std::fs::write(corpus.join(caso).join("input.js"), &js).unwrap();
    let args = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1", "--carril", "b", "--sandbox"];
    let cli = crate::Cli::parse(args.iter().map(|s| s.to_string()).collect()).unwrap();
    let preload = Preload::write_tagged("sandbox-modulo").unwrap();
    let known = crate::known::Known::load(&corpus, &out, 0, Minifier::Oxc);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    let celda = out.join(cli.cell());
    let reporter = crate::oracle::Reporter::new(&celda, cli.cell(), crate::runner::PRELOAD);
    let runner = crate::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
    let cfg = Config::from_backend(Box::new(Cambia { de: "bug-no".into(), a: "bug-si".into(), kind: SourceKind::Module }));
    let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
    let mut st = crate::Stats::default();
    crate::process_case(&ctx, &runner, &cfg, None, &cli, &mut st, &corpus.join(caso).join("input.js"));
    drop(runner);
    let meta = std::fs::read_to_string(celda.join("div").join(caso).join("meta.txt")).unwrap_or_default();
    assert_eq!(meta.matches("solo el original toco el sandbox").count(), 1, "{} | {meta}", st.line());
    assert_eq!((st.solo_a_sandbox, st.a_sandbox), (2, 0), "{}", st.line());
    drop(preload);
    let _ = std::fs::remove_dir_all(&b);
}

/// 7.9 (revision fe0cd86, MENOR 3): carril B, un modulo mudo cuyo original y minificado tocan
/// el sandbox (leen negado y lo atrapan en silencio) y que tiene un bug ajeno al sandbox
/// (`revision/a3-2c/corpus-b/m02-ayb-bug-real`): la pasada cruda (iguales: los dos mudos) y el
/// envoltorio de modulos son A-y-B y se comparan. El hallazgo (div/) lleva la nota UNA vez y el
/// caso cuenta dos pasadas A-y-B; nada va a descartes.
#[test]
fn sandbox_ambos_modulo() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("ambos-modulo");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    let caso = "m02-ayb-bug-real";
    let js = format!("import fs from \"node:fs\";\ntry {{ fs.readFileSync({}); }} catch (e) {{}}\nexport const s = \"bug-no\";\n", js_str(&senuelo));
    std::fs::create_dir_all(corpus.join(caso)).unwrap();
    std::fs::write(corpus.join(caso).join("input.js"), &js).unwrap();
    let args = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1", "--carril", "b", "--sandbox"];
    let cli = crate::Cli::parse(args.iter().map(|s| s.to_string()).collect()).unwrap();
    let preload = Preload::write_tagged("sandbox-ambos-modulo").unwrap();
    let known = crate::known::Known::load(&corpus, &out, 0, Minifier::Oxc);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    let celda = out.join(cli.cell());
    let reporter = crate::oracle::Reporter::new(&celda, cli.cell(), crate::runner::PRELOAD);
    let runner = crate::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
    let cfg = Config::from_backend(Box::new(Cambia { de: "bug-no".into(), a: "bug-si".into(), kind: SourceKind::Module }));
    let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
    let mut st = crate::Stats::default();
    crate::process_case(&ctx, &runner, &cfg, None, &cli, &mut st, &corpus.join(caso).join("input.js"));
    drop(runner);
    let meta = std::fs::read_to_string(celda.join("div").join(caso).join("meta.txt")).unwrap_or_default();
    assert_eq!(meta.matches("el original y el minificado tocaron el sandbox").count(), 1, "{} | {meta}", st.line());
    assert_eq!((st.ambos_sandbox, st.a_sandbox, st.divergences), (2, 0, 1), "{}", st.line());
    drop(preload);
    let _ = std::fs::remove_dir_all(&b);
}
