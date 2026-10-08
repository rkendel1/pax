# PAX observation (`pax observe`)

Schema: `pax.observation.v1`. Code: `src/observe.rs`. Tests: `tests/observe.rs`.

**PAX provides deterministic project observation. It does not determine
relevance, intent, plans, correctness, goal satisfaction, or change safety.**

> PAX observes project reality. Chip decides what to do with it. FX reasons about
> what observations mean.

`pax observe` reports project-structure facts that can be established directly
from project artifacts and Rust source files, each with provenance. A consumer
(Chip's capability adapter, a human, a script) decides what the facts mean. PAX
has no knowledge of goals, work state, prompts, escalation, Attn, FeltDB, AppPort,
or Compute, calls no model, retries nothing, writes nothing, and keeps no cache,
index, or background process.

## Declared, observed, resolved, verified

| Strength | Meaning | Example in this command |
| --- | --- | --- |
| `declared` | A project artifact states it. Nothing checked it. | `dependency.declared serde ^1`; `mod foo;` whose file was not found |
| `observed` | PAX read the artifact on this machine, now. | `src/foo.rs` exists; a `struct` is written at `src/foo.rs:12` |
| `resolved` | A resolver established it. | workspace membership from `cargo metadata` |
| `verified` | Execution established it. | **never emitted**; `observe` runs nothing |

Strength is never upgraded without evidence: presence is not resolution, and
resolution is not verification. A dependency stays `declared` even when a lockfile
exists. A `mod foo;` is `observed` only after its file was found by Rust's path
rules and read; otherwise it stays `declared` and a diagnostic says why. The
`verified` strength does not exist in this schema.

## Usage

```text
pax [--json] observe [--scope <scope>] [--max-files <n>] [--max-bytes <n>] [--max-facts <n>]
```

| Scope | Meaning |
| --- | --- |
| `project` (default) | root artifacts, workspace members, declared dependencies, and the module tree of every member crate |
| `crate:<package>[/<kind>[/<name>]]` | one workspace package, or one target of it (`a/lib`, `a/bin/tool`, `a/test/it`) |
| `module:<crate-id>::crate[::<mod>...]` | one module subtree; only the path to it is walked |
| `file:<path>` | one `.rs` file, read in isolation |
| `path:<prefix>` | `.rs` files and manifests under a path prefix (`path:.` for the whole project; an empty value is invalid) |

Crate ids are `<package>/lib` or `<package>/<bin\|test\|bench\|example>/<name>`.
Module ids are `<crate-id>::crate::<mod>::<mod>`.

## Bounds

Limits must be positive integers. Defaults: `--max-files 200`, `--max-bytes 4194304`, `--max-facts 5000`.

- `max_files` bounds *both* files read and directory entries listed or probed.
- `max_bytes` bounds source bytes read (checked before each read).
- `max_facts` bounds output size.

Exceeding a bound is a typed error (`limit_exceeded`, with the limit, its value,
and the cost spent), never a truncated result and never a wider scan. Source
files are reached by following `mod` declarations from crate roots, not by
walking the tree, so unreferenced and vendored files are not touched in
`project`, `crate`, or `module` scopes. Only `path:` walks directories (skipping
dot-directories, `target`, `node_modules`). `project` scope does not use
`detect_repository`'s directory walk.

Every result reports its own cost: entries listed, files inspected and parsed,
bytes read, facts, elapsed milliseconds. Measured on `chip-rs` (debug build, 21 crates):
`crate:chip-graph` 10 files / 91 KB / 170 facts / 44 ms; `crate:chip-core` 50 files / 573 KB /
1031 facts / 173 ms; whole project 193 files / 2.5 MB / 4288 facts (1.9 MB JSON) / 733 ms
and refused under the default bounds. Narrow scopes are the intended use.

## Output

```jsonc
{
  "schema": "pax.observation.v1",
  "status": "ok" | "partial",          // partial = at least one diagnostic
  "project": { "root": "...", "name": "..." },
  "scope":   { "kind": "module", "value": "pax/lib::crate::a" },
  "observed_at": 1791469317,           // unix seconds
  "tool":    { "name": "pax", "version": "...", "parser": "syn 2 (full)" },
  "limits":  { "max_files": 200, "max_bytes": 4194304, "max_facts": 5000 },
  "cost":    { "entries_listed": 0, "files_inspected": 0, "files_parsed": 0,
               "bytes_read": 0, "facts": 0, "elapsed_ms": 0 },
  "facts": [{
    "relationship": "declaration.located_at",
    "subject": { "type": "declaration", "id": "pax/lib::crate::a::f",
                 "attributes": { "kind": "fn", "visibility": "pub", "module": "pax/lib::crate::a" } },
    "object": null,
    "location": { "path": "src/a.rs", "line": 3 },
    "provenance": { "strength": "observed", "method": "syn.parse_file", "source": "src/a.rs" }
  }],
  "diagnostics": [{ "code": "syntax_error", "state": "unparseable", "message": "...", "location": { "path": "...", "line": 1 } }]
}
```

Errors are one JSON document with `"status": "error"`, a `code`, and for limits
`limit`, `limit_value`, and `cost`. They exit with code 2 and, like other PAX
errors, go to stderr. Without `--json`, errors are `error[code]: message` and
facts are one line each.

### Relationships

| Relationship | Strength | Method | Notes |
| --- | --- | --- | --- |
| `artifact.exists` | observed | `fs.stat` | manifests, lockfile, conventional dirs (`src tests benches examples docs`), files |
| `workspace.member` | resolved (declared if `cargo metadata` failed) | `cargo.metadata` | only when `[workspace]` is declared |
| `dependency.declared` | declared | `cargo.metadata` | `--no-deps`; `kind` is `dependency`/`development`/`build`; aggregated across members (not attributed to a package); `project` scope only |
| `crate.contains` | observed | `cargo.target-convention` | crate -> root module; root file found by Cargo's directory convention |
| `module.contains` | observed, or declared when the module file was not established | `syn.parse_file` | module -> child module |
| `module.located_at` | observed | `rust.module-file-rule`, `syn.parse_file`, `cargo.target-convention` | file (and line for inline modules) |
| `declaration.located_at` | observed | `syn.parse_file` | `kind`, `visibility`, containing `module` as attributes |
| `test.declared` | observed | `syn.parse_file` | a fn with an attribute whose last path segment is `test` |

Containment of a declaration in a module is the `module` attribute of
`declaration.located_at`, not a separate fact. Source parsing is `syn`
(`full`), a real Rust parser, with no heuristics. Module files are found by
Rust's path rules applied by PAX, not by `rustc`.

### Diagnostics

States are `unsupported`, `unparseable`, `unreadable`, `unresolved`. Codes:
`syntax_error`, `not_utf8`, `artifact_unreadable`, `mod_path_attribute`,
`custom_target_path`, `artifact_outside_root`, `module_file_missing`, `module_file_ambiguous`,
`module_file_revisited`, `source_structure_unsupported`, `no_supported_project`,
`cargo_metadata_unavailable`. A diagnostic means PAX did not establish something
and said so; it never falls back to guessed structure.

Error codes: `invalid_scope`, `invalid_limit`, `unsupported_scope`,
`unsupported_project`, `scope_not_found`, `limit_exceeded`, `artifact_unreadable`.

## What the first implementation does not observe

- Any language other than Rust. Other ecosystems get `artifact.exists` for their
  manifest and a `source_structure_unsupported` diagnostic.
- `impl` blocks and their methods, `use`/imports, calls, types, traits
  implemented, generics, and re-exports. `import` and "who calls this" are not
  answered.
- Items produced by macros, `build.rs`, or `include!`.
- `cfg` evaluation. `cfg(...)` tokens are reported on the item and never
  evaluated; cfg-disabled modules are still walked.
- `#[path]` modules and custom `[lib]`/`[[bin]]`/`[[test]]` paths (reported as
  unsupported). Only Cargo's conventional target layout is read.
- Test-to-source relationships, behavioral coverage, and test results. A
  `test.declared` fact says an attribute is present, not what the test proves.
- Dependency resolution or installation. No lockfile is parsed. Dependencies are
  not attributed to individual packages and are not reported in `crate`/`module`
  scopes.
- Anything about relevance, impact, safety, correctness, or what to do next.

## Compatibility

`observe` is additive. `pax graph` and the existing commands, `--json` output,
`pax --json test` (`pax.execution-result.v1`), project detection, and
runtime/reality fields are unchanged. `--scope`, `--max-files`, `--max-bytes`,
and `--max-facts` are rejected on every other command. `observe` runs no
package manager, runtime, or project command; like the other observation
commands it starts `cargo metadata --no-deps --offline` for Cargo projects (not
for `file:` and `path:` scopes). It is covered by the read-only test in
`tests/boundary.rs`. Two new dependencies are used for parsing: `syn` (already in
`Cargo.lock` via `serde_derive`) and `proc-macro2` (line numbers).

`observed_at` and `cost.elapsed_ms` are the only nondeterministic fields; the
rest is stable for unchanged inputs.

## Consuming from Chip

Chip should call `pax --json observe --scope ...` through its own capability
adapter (a process boundary), parse by `schema`, and treat `facts` as inputs to
its own decision. It should not depend on PAX in `chip-core`, and should treat an
unknown `schema` value, a `status` of `error`, or `partial` diagnostics as
explicit states, not as empty results.

## Evaluation (not yet done)

Whether this reduces Chip's rediscovery cost is an empirical question that this
change does **not** answer. Only cost per observation has been measured (above);
no model was run. Before claiming value, run matched real-model tasks:

- **Baseline:** model -> list/search/read -> interpret -> act.
- **Treatment:** model -> `pax observe` -> interpret -> act.
- **Measure:** model calls, executions, tokens/context, verified useful work,
  elapsed time, observation cost, safety outcomes, false completions. The
  headline numbers are Chip's own `verified_outputs / model_calls` and
  `verified_outputs / executions`; PAX defines no competing measure.
- **Reject or narrow** if matched tasks do not reduce rediscovery or model calls,
  context use does not improve, observation cost exceeds savings, reliability
  drops, provenance proves ambiguous, unsupported constructs are guessed, or the
  capability starts encoding relevance or needs goals or plans.
