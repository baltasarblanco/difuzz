# difuzz

A differential fuzzer for JavaScript minifiers, written in Rust. For each case of a corpus,
difuzz minifies the program, runs the original and the minified program under `node`, and
compares what they do. If they differ, the minifier changed the program's behavior: that is a
candidate miscompilation.

It has found miscompilations in both supported minifiers that were reported upstream.

The full manual, the `--help` text, the program's output and the code comments are in Spanish:
see [MANUAL.md](MANUAL.md).

## How it works

1. **Corpus**: every `input.js` under the directory you pass, in a fixed order.
2. **Minify**: the backend parses the case once and prints two programs with the same printer:
   the original, unoptimized, and the minified one. A difference can then only come from the
   optimizer, not from the parser or the printer (`--crudo` also runs the raw text, to cover
   the parser and the printer).
3. **Run**: both programs run under node with a preload that freezes non-deterministic APIs
   (`Date`, `Math.random`, ...), reports uncaught exceptions, prints the top-level bindings of
   programs that print nothing, and flags uses of what the minifier documents as assumed (for
   example, relying on `Function.prototype.toString`).
4. **Compare**: exit status, stdout and what the program writes to stderr.
5. **Classify**: each confirmed divergence goes to `div/` (a finding) or to `known/` (explained
   by a documented assumption of the minifier).

| `--minifier` | crate | pinned version |
|---|---|---|
| `oxc` (default) | `oxc_minifier` | `=0.153.0` |
| `swc` | `swc_ecma_minifier` | `61.0.8` (`Cargo.lock`) |

## Install

Requirements: Linux, Rust and cargo (checked with rustc 1.98.0), and `node` on `PATH`
(checked with Node 24.14.1). Other systems have not been tested.

difuzz is not published on crates.io; build it from source:

```sh
git clone https://github.com/baltasarblanco/difuzz
cd difuzz
cargo build --release && cargo build --profile oraculo
```

This builds `target/release/difuzz`, the binary you run, and `target/oraculo/difuzz`, the same
code without debug assertions. difuzz uses the second one when a minifier fails a
`debug_assert!`, so that a development-only assertion does not hide a miscompilation that the
npm package does produce. Rebuild both together.

## Example

The repository has a small corpus that is used to check difuzz itself:

```sh
target/release/difuzz tests/golden/corpus --out out --jobs 4
```

The summary goes to stdout (`total=19 iguales=... DIVERGENCIAS=...`) and progress to stderr.
Each result directory is in `out/<cell>/` (here `out/oxc+carril_a+tal_cual/`): `div/` holds
the findings, one directory per case, with `meta.txt` (what differs and the commands to
reproduce it), the original and minified programs, and what each one printed.
`CORRIDA.txt` says whether the run finished.

Other useful options: `--minifier swc`, `--config core_defaults` (the npm package's defaults),
`--carril b` (run as an ES module), and `--emitir FILE` (print the original and the minified
program without running node). `difuzz --help` lists all of them.

## Sandbox limits

difuzz executes every corpus case. **By default node runs without a sandbox**: a case can read
and write whatever your user can, spawn processes and use the network.

- `--sandbox` uses the Node permission model plus a seccomp filter. The permission model is not
  a barrier against hostile code, and it does not block the network or unix sockets.
- `--sandbox-red` (needs unprivileged user namespaces) adds network and mount namespaces. It
  reduces the exposure, but it is **not a security boundary**.

For a corpus you do not trust, run difuzz in a disposable VM or container and use
`--sandbox-red` inside it as well. A hostile case can also forge its own results. Details and
known limits are in [SECURITY.md](SECURITY.md).

## Documentation

- [MANUAL.md](MANUAL.md) (Spanish): how difuzz works, every option, the output, the
  classification, the sandbox, node's limits, exit codes and the corpora.
- [SECURITY.md](SECURITY.md): the sandbox model, its limits, and how to report a vulnerability.
- [CONTRIBUTING.md](CONTRIBUTING.md): building, checking a change, commits.
- [tests/README.md](tests/README.md) (Spanish): the test suites.

## AI assistance

difuzz is built with Anthropic's Claude as a coding assistant. The author sets the design,
reviews every change and runs the checks before anything is merged.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Some test files come from other projects and keep their licenses: cases from
[SWC](https://github.com/swc-project/swc) (Apache-2.0, see
[tests/muestra/LICENSE-SWC](tests/muestra/LICENSE-SWC)) and from
[terser](https://github.com/terser/terser) (BSD-2-Clause, see
[tests/muestra/LICENSE-TERSER](tests/muestra/LICENSE-TERSER)). Their origin is described in
[tests/muestra/README.md](tests/muestra/README.md), [tests/golden/README.md](tests/golden/README.md)
and [tests/README.md](tests/README.md).
