# Security

## difuzz runs untrusted code

difuzz executes every corpus case under Node.js: the original and the minified program, plus
extra runs for confirmations and re-checks. If the corpus is not yours (npm packages, code from
the web, cases someone else generated), a run executes that code as the user running difuzz.

## Sandbox

By default node runs **without a sandbox**: a case can read and write whatever your user can,
spawn processes and use the network.

- `--sandbox`: every node run that executes a case uses the Node permission model
  (`--permission`; checked with Node 24.14.1). Reads are allowed only for the preload, the case
  (and difuzz's module wrapper), the stubs manifest with `--stubs`, and the case's working
  directory; writes only for that directory and difuzz's control channel. No child processes,
  workers, native addons or WASI, and `node:sqlite` is disabled (`--no-experimental-sqlite`).
  A seccomp filter (Linux x86_64 and aarch64 only; on other architectures `--sandbox` refuses
  to start) lets node send signals only to itself and change only its own priority.
- `--sandbox-red` (Linux only, no root, needs user namespaces): `--sandbox` plus user, network
  and mount namespaces. Only a private loopback is reachable, `/run` is hidden behind a tmpfs,
  and the system's unix sockets outside `/run` are covered with `/dev/null`. If it cannot be set
  up, difuzz exits with 1 before running anything.
- `scripts/barrido-real.sh` always uses `--sandbox-red`.
- Every node run that executes a case has a timeout (`--timeout-ms`, 5000 ms by default).

**`--sandbox-red` reduces the exposure, but it is not a security boundary.** It cuts the network
and most local services, but access to your files is enforced only by the Node permission
model, which is not a barrier against hostile code, and with the default backend almost every
run also allows the inspector (see Known limits). For a corpus you do not trust, run difuzz
inside a disposable VM or container that holds nothing you need to protect, and use
`--sandbox-red` inside it as well (it needs unprivileged user namespaces; where it cannot create
them, difuzz exits with 1 before running anything, and `--sandbox` is the fallback).

## Known limits

- The Node permission model is **not a barrier against hostile code**. With `--sandbox` alone
  it does not block the network or `connect` to a unix socket, so a case can reach D-Bus,
  systemd-resolved or any other service with a socket.
- With the oxc backend (the default), almost every node run (the original, the minified
  program, confirmations and re-checks) uses `--allow-inspector`, because difuzz's violation
  sensor needs it; with the swc backend, only the coverage runs of the re-checks do. It lets the
  program open an inspector session in its own process, and Node warns that this may
  invalidate the permission model. difuzz blocks the ways it knows to open an inspector port
  (the preload replaces `inspector.open`, and node runs with `--disable-sigusr1`).
- With `--sandbox-red`, unix sockets created after node starts, those with spaces in their
  path and those bound with a relative path are not covered and stay reachable. The rest of
  the filesystem is still visible to the operating system; only the Node permission model
  restricts it, not the namespace.
- In Node 24.14.1, `node:sqlite` does not go through the permission model (that is why the
  sandbox disables it). The other Node modules have not been audited against the permission
  model.
- Apart from the timeout, node runs have no memory or CPU limit of their own (only V8's
  default heap limit). `--mem-hijo` limits the minifier and analysis child processes, not node.
- `node --check` (difuzz uses it to ask whether node accepts a source) and `node --version` run
  outside the sandbox, without a timeout and with difuzz's environment. `node --check` only
  checks the syntax; it does not run the case.
- By design, a case runs in the same Node process as difuzz's preload, which records the case's
  results through difuzz's control channel, and with `--sandbox` the case is allowed to write to
  that channel. So a hostile case can forge its own results: difuzz's findings on a hostile
  corpus are not trustworthy, even when the sandbox holds. (Inferred from the code; no exploit
  has been tried.)
- The `repro_original:` / `repro_minificado:` commands written to `meta.txt` do not include the
  sandbox flags: running them by hand runs the case without the sandbox.

The Sandbox section of `README.md` (in Spanish) has the full details and measurements.

## Reporting a vulnerability

If you find a vulnerability in difuzz itself (for example, a way for a corpus case to affect
difuzz's own process or files outside the case's working directory, or a sandbox flag that does
less than this file says),
please report it privately through GitHub's private vulnerability reporting ("Report a
vulnerability" in the repository's Security tab) instead of opening a public issue.
<!-- VERIFICAR: private vulnerability reporting can only be enabled on a public repository (Settings > Security); enable it right after publishing. -->

Miscompilations that difuzz finds in a minifier are not vulnerabilities in difuzz: report them
to that minifier's project.
