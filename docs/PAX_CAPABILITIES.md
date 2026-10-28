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
promoted to a confirmed `drift`. `--live` now performs a bounded, read-only
Docker/Compose runtime observation rather than naming a command it never ran.

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
| Runtime observation (`--live`) | ✅ Docker/Compose only: locates `docker`, probes the daemon with `docker info`, and runs bounded `docker compose -f <file> ps --format json`; reports scope, RFC 3339 UTC time, permission, commands, and failures; missing CLI/daemon, failure, and absence are distinct | keep read-only; no start/stop/build/pull | PAX | — |
| Tool detection (is `npm`/`cargo` on PATH, version) | ❌ not implemented (only `cargo` is invoked for metadata) | observation only, with path and version | PAX | P2 |
| Runtime/platform requirement extraction (`engines`, `requires-python`, `rust-version`, `.nvmrc`) | ❌ none read | declared-requirement observation | PAX | P1 |
| Project structure observation (`pax observe`: Rust crate/module/file/declaration, bounded, per-fact provenance) | ✅ Rust only; see [PAX_OBSERVATION.md](PAX_OBSERVATION.md) for what it does not observe | keep narrow; no relevance or impact semantics | PAX | — |
| Declared vs. observed distinction | ✅ separate `declared`/`lockfile`/`installed`/`runtime` facts per dependency in `reality`; evidence `kind` in `drift`; not in `info`/`deps` | uniform per-field provenance | PAX | P1 |
| Observation provenance (what, where, how) | ✅ file lists for components, graph, reality, drift; every dependency layer names its `source`; diagnostics name a check | per-field source references | PAX | — |
| Observation time | ⚠️ runtime only: `--live` records `runtime.observedAt`; static fields remain timestamp-free and deterministic | optional, separable from semantic fields | PAX | P2 |
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

## Execution semantics

PAX delegates execution operations to native tools. When no valid mapping exists, PAX returns a typed `unsupported` result with a specific reason code.

| Operation | Rust (Cargo) | JavaScript | Python | Docker/Compose |
|-----------|--------------|------------|--------|----------------|
| `build` | `cargo build` | package-manager `build` script | `uv build` / `poetry build` / `pdm build` (requires [build-system]) | `docker compose build` or `docker build` |
| `test` | `cargo test` (interpretation via `--message-format=json`) | package-manager `test` script | `uv run pytest` / `poetry run pytest` (requires pytest config) | ❌ unsupported: use `pax run <service>` instead |
| `lint` | `cargo clippy` | package-manager `lint` script | `uv run ruff check` / `poetry run ruff check` (requires ruff config) | ❌ unsupported: use `pax exec docker compose ...` instead |
| `typecheck` | `cargo check` | package-manager `typecheck` script | `uv run mypy` / `poetry run mypy` (requires mypy config) | ❌ unsupported: use `pax exec docker compose ...` instead |
| `install` | `cargo fetch` | `npm install` / `pnpm install` / `yarn install` / `bun install` | `uv sync` / `poetry install` / `pdm install` / `pip install -r requirements.txt` | `docker compose pull` (Compose only) |
| `add` | `cargo add` | `npm add` / `pnpm add` / `yarn add` / `bun add` | `uv add` / `poetry add` / `pdm add` | ❌ unsupported: use `pax exec docker ...` instead |
| `remove` | `cargo remove` | `npm remove` / `pnpm remove` / `yarn remove` / `bun remove` | `uv remove` / `poetry remove` / `pdm remove` | ❌ unsupported: use `pax exec docker ...` instead |
| `run` | `cargo run` | `npm run <script>` | `uv run <cmd>` / `poetry run <cmd>` | `docker compose run <service>` or `docker compose <operation>` |
| `x` | `cargo <tool>` | `npm exec` / `pnpm exec` / `yarn dlx` / `bunx` | `uvx` / `pipx` | ❌ unsupported: use `pax exec docker ...` instead |

**Notes:**
- **JavaScript**: PAX uses the package-manager specified in `packageManager` field or lockfile precedence (`npm` > `pnpm` > `yarn` > `bun`). Scripts must exist in `package.json` for `build`, `test`, `lint`, `typecheck`.
- **Python**: Operations require both tool availability and project configuration (e.g., pytest in pyproject.toml for `test`). Pipenv is discovered but unsupported for execution.
- **Docker**: Only `build` and `install` (pull) have native mappings. `run` accepts Compose operations or service names.
- **Unsupported result**: When no mapping exists, PAX returns `outcome: unsupported` with `reason` in `executionResult` (see PAX_CONTRACT.md).

## Commands

| Command | Class | Reads | Starts a process | Writes |
| --- | --- | --- | --- | --- |
| `info`, `doctor`, `deps`, `scripts`, `workspaces`, `lock` | inspection | manifests, lockfiles | `cargo metadata` (Cargo projects) | nothing |
| `graph`, `reality`, `drift` (+`--live`) | observation | same, plus lockfile parsing, installed-package enumeration, and Compose service declarations | `cargo metadata` (Cargo projects); `--live` also runs bounded `docker info` and `docker compose ps` | nothing |
| `--dry-run [--json] <operation>` | plan | same | none (Cargo metadata aside) | nothing |
| `build`, `test`, `lint`, `typecheck`, `run`, `x`, `exec`, `install`, `add`, `remove`, `deploy` | delegation | same | the selected native tool | whatever that tool writes |
| `--json test` | delegation + interpretation | same | `cargo test`, or the selected tool | whatever that tool writes |
