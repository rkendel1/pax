# PAX capability matrix

Verified against the code and tests at v0.3.0. Legend: ✅ implemented and
tested, ⚠️ partial or heuristic (see notes), ❌ not implemented. "Current" is
what the code does, not what the README says. Details and evidence:
[PAX_AUDIT.md](PAX_AUDIT.md). Boundary: [PAX_BOUNDARY.md](PAX_BOUNDARY.md).

v0.5.0 note: the schema/versioning row below describes the v0.3.0 baseline.
Canonical schema identifiers, typed outcomes, project-relative graph
identities, and the compatibility policy are now defined in
[PAX_CONTRACT.md](PAX_CONTRACT.md). Discovery and declaration parsing were
corrected in 0.5.0: Python TOML/requirements/Pipfile are parsed, nested
`package.json` manifests are components, malformed manifests produce
diagnostics, Cargo workspaces are read from TOML, and Compose services are
limited to `services:` children. Pipenv is discovered but not executable.
Dependency evidence is now layered and reconciled: recognized lockfiles are
parsed, installed package names are enumerated, and unknown evidence is never
promoted to a confirmed `drift`.

## Capabilities by owner

| Capability | Current | Intended | Owner | Priority |
| --- | --- | --- | --- | --- |
| Project discovery (root + nested dirs, skipping dot-dirs, `target`, `node_modules`) | ✅ | defined | PAX | — |
| Ecosystem detection (JS, Python, Rust, Container) | ✅ every recognized manifest is a component; malformed manifests produce Error diagnostics and `partial` | defined, with explicit unparseable/unknown states | PAX | — |
| Package-manager selection (JS: `packageManager` > lockfile precedence; Python: lockfile; `--tool`) | ✅ deterministic, fail-closed on conflict | keep | PAX | — |
| Dependency inspection: package.json | ✅ root groups plus nested manifests as sourced native dependencies | keep | PAX | — |
| Dependency inspection: Cargo | ✅ via `cargo metadata`; ⚠️ TOML fallback (labelled `source`) | keep | PAX | — |
| Dependency inspection: Python | ✅ TOML (`[project]`, optional groups, `[dependency-groups]`, Poetry, build requires), `requirements*.txt` and `requirements/*.txt`, Pipfile | keep; pipenv execution stays unsupported | PAX | — |
| Script inspection (package.json `scripts`) | ✅ | keep | PAX | — |
| Workspace inspection (npm/pnpm, Cargo) | ✅ Cargo workspace membership from metadata or TOML | keep | PAX | — |
| Container inspection (Dockerfile directives, Compose) | ✅ Compose services are direct children of `services:`; Dockerfile directives are line-scanned | keep | PAX | — |
| Lockfile inspection | ✅ recognized formats are parsed into package sets (npm v1/v2/v3, pnpm, yarn v1/Berry, bun JSONC, Cargo, uv, poetry, pdm, Pipfile.lock); unparseable/empty/unrecognized files are `unknown` | parse + explicit unknown | PAX | — |
| Installed-state observation | ✅ JS enumerates `node_modules/` package names (empty = `absent`); Python enumerates `site-packages/` when a conventional environment exists; Rust `target/` is build output and installed state is `unknown` | names + explicit unknown | PAX | — |
| Runtime observation (`--live`) | ❌ names `docker compose ps`, runs nothing | implement or remove the flag's claim | PAX (if kept) | **P0** (honesty) |
| Tool detection (is `npm`/`cargo` on PATH, version) | ❌ not implemented (only `cargo` is invoked for metadata) | observation only, with path and version | PAX | P2 |
| Runtime/platform requirement extraction (`engines`, `requires-python`, `rust-version`, `.nvmrc`) | ❌ none read | declared-requirement observation | PAX | P1 |
| Project structure observation (`pax observe`: Rust crate/module/file/declaration, bounded, per-fact provenance) | ✅ Rust only; see [PAX_OBSERVATION.md](PAX_OBSERVATION.md) for what it does not observe | keep narrow; no relevance or impact semantics | PAX | — |
| Declared vs. observed distinction | ✅ separate `declared`/`lockfile`/`installed`/`runtime` facts per dependency in `reality`; evidence `kind` in `drift`; not in `info`/`deps` | uniform per-field provenance | PAX | P1 |
| Observation provenance (what, where, how) | ✅ file lists for components, graph, reality, drift; every dependency layer names its `source`; diagnostics name a check | per-field source references | PAX | — |
| Observation time | ❌ none (and intentionally deterministic) | optional, separable from semantic fields | PAX | P2 |
| Confidence / unknown states | ✅ `unknown` in `reality` and `drift`; unknown evidence never promotes to confirmed `drift` | uniform | PAX | — |
| Stable schema/versioning | ⚠️ `schemaVersion: "1"` (info family) vs `schema_version: "1"` (graph/reality/drift); no compatibility policy; execution result `pax.execution-result.v1` is the only self-describing contract | one convention + written policy | PAX | P1 |
| Normalized project identity | ❌ `project.name` falls back to directory name; `project.root` is an absolute machine path | stable identity without machine paths | PAX | P2 |
| Tool selection / planning (`--dry-run`) | ✅ | keep | PAX | — |
| Delegated native execution (`build`, `test`, `run`, `exec`, …) | ✅ | keep as thin delegation; not a workload runner | PAX (delegation only) | — |
| Test result interpretation (`--json test`, Cargo) | ✅ `pax.execution-result.v1` | keep; other ecosystems report `unsupported` | PAX | — |
| Test result interpretation (npm/pytest/…) | ❌ runs tool, reports `unsupported` | only if a native structured source exists | PAX | P3 |
| Additional ecosystems (Go, Java, .NET, …) | ❌ | only with a concrete consumer | PAX | P3 |
| Dependency resolution | ❌ (correctly) | stay delegated | package managers | — |
| Package installation | ⚠️ delegated only (`install`/`add`/`remove`) | stay delegated | package managers | — |
| Environment creation | ❌ (correctly) | outside PAX | Compute / other | — |
| Build / artifact production | ⚠️ delegated only | stay delegated | build systems | — |
| Execution, placement, isolation | ❌ in PAX (correctly); ⚠️ delegation wrapper exists | Compute | Compute | — |
| Deployment | ⚠️ delegated only (`fly`, `vercel`, `netlify`) | consider removing from PAX | other | P2 (decision) |
| Provenance: project files | ⚠️ file paths only | paths + (optionally) content hashes | PAX | P2 |
| Provenance: revision identity | ❌ | outside PAX | Git | — |
| Provenance: tool/binary identity | ❌ | outside PAX | Compute / trust layer | — |
| Execution evidence / receipts | ❌ in PAX (correctly) | Compute | Compute | — |
| Durable state | ❌ in PAX (correctly) | FeltDB | FeltDB | — |
| Agent reasoning, planning, repair | ❌ in PAX (correctly) | Chip | Chip | — |
| Goals, approvals, attention | ❌ in PAX (correctly) | Attn | Attn | — |

Priorities: **P0** = output is misleading today; **P1** = needed for an honest
contract; **P2** = useful with a concrete consumer; **P3** = defer.

## Commands

| Command | Class | Reads | Starts a process | Writes |
| --- | --- | --- | --- | --- |
| `info`, `doctor`, `deps`, `scripts`, `workspaces`, `lock` | inspection | manifests, lockfiles | `cargo metadata` (Cargo projects) | nothing |
| `graph`, `reality`, `drift` (+`--live`) | observation | same, plus lockfile parsing and installed-package enumeration | `cargo metadata` (Cargo projects) | nothing |
| `--dry-run [--json] <operation>` | plan | same | none (Cargo metadata aside) | nothing |
| `build`, `test`, `lint`, `typecheck`, `run`, `x`, `exec`, `install`, `add`, `remove`, `deploy` | delegation | same | the selected native tool | whatever that tool writes |
| `--json test` | delegation + interpretation | same | `cargo test`, or the selected tool | whatever that tool writes |
