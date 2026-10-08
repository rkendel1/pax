# PAX capability matrix

Verified against the code and tests at v0.3.0. Legend: ✅ implemented and
tested, ⚠️ partial or heuristic (see notes), ❌ not implemented. "Current" is
what the code does, not what the README says. Details and evidence:
[PAX_AUDIT.md](PAX_AUDIT.md). Boundary: [PAX_BOUNDARY.md](PAX_BOUNDARY.md).

## Capabilities by owner

| Capability | Current | Intended | Owner | Priority |
| --- | --- | --- | --- | --- |
| Project discovery (root + nested dirs, skipping dot-dirs, `target`, `node_modules`) | ✅ | defined | PAX | — |
| Ecosystem detection (JS, Python, Rust, Container) | ⚠️ file-presence based; no diagnostic for unparseable manifests | defined, with explicit unparseable/unknown states | PAX | P1 |
| Package-manager selection (JS: `packageManager` > lockfile precedence; Python: lockfile; `--tool`) | ✅ deterministic, fail-closed on conflict | keep | PAX | — |
| Dependency inspection: package.json | ✅ | keep | PAX | — |
| Dependency inspection: Cargo | ✅ via `cargo metadata`; ⚠️ heuristic fallback (labelled `source`) | keep | PAX | — |
| Dependency inspection: Python | ❌ pyproject reports project metadata as dependencies; `requirements.txt` alone yields none; no `requires-python` | correct parser or report "not interpreted" | PAX | **P0** |
| Script inspection (package.json `scripts`) | ✅ | keep | PAX | — |
| Workspace inspection (npm/pnpm, Cargo) | ✅ | keep | PAX | — |
| Container inspection (Dockerfile directives, Compose) | ⚠️ line heuristics; Compose keys such as `ports` reported as services | correct or label as heuristic | PAX | P2 |
| Lockfile inspection | ⚠️ presence only; drift uses substring search for names | presence + explicit "not parsed" | PAX | P1 |
| Installed-state observation | ⚠️ directory presence only (empty `node_modules/` = present; `target/` is not an install) | rename/limit claims | PAX | P1 |
| Runtime observation (`--live`) | ❌ names `docker compose ps`, runs nothing | implement or remove the flag's claim | PAX (if kept) | **P0** (honesty) |
| Tool detection (is `npm`/`cargo` on PATH, version) | ❌ not implemented (only `cargo` is invoked for metadata) | observation only, with path and version | PAX | P2 |
| Runtime/platform requirement extraction (`engines`, `requires-python`, `rust-version`, `.nvmrc`) | ❌ none read | declared-requirement observation | PAX | P1 |
| Project structure observation (`pax observe`: Rust crate/module/file/declaration, bounded, per-fact provenance) | ✅ Rust only; see [PAX_OBSERVATION.md](PAX_OBSERVATION.md) for what it does not observe | keep narrow; no relevance or impact semantics | PAX | — |
| Declared vs. observed distinction | ⚠️ layers in `reality`; evidence `kind` in `drift`; not in `info`/`deps` | uniform `declared`/`observed` provenance per field | PAX | P1 |
| Observation provenance (what, where, how) | ⚠️ file lists for components, graph, reality; none for diagnostics, manager version | per-field source references | PAX | P1 |
| Observation time | ❌ none (and intentionally deterministic) | optional, separable from semantic fields | PAX | P2 |
| Confidence / unknown states | ⚠️ `unknown` in `reality` and live `drift` only | uniform | PAX | P1 |
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
| `graph`, `reality`, `drift` (+`--live`) | observation | same, plus directory existence checks | `cargo metadata` (Cargo projects) | nothing |
| `--dry-run [--json] <operation>` | plan | same | none (Cargo metadata aside) | nothing |
| `build`, `test`, `lint`, `typecheck`, `run`, `x`, `exec`, `install`, `add`, `remove`, `deploy` | delegation | same | the selected native tool | whatever that tool writes |
| `--json test` | delegation + interpretation | same | `cargo test`, or the selected tool | whatever that tool writes |
