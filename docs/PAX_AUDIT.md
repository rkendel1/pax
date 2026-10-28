# PAX audit

Scope: the PAX repository at v0.3.0 (`src/lib.rs`, `src/execution_result.rs`,
`src/main.rs`, `tests/`), and the consumers found under `~/Developer`:
Compute (`compute-project/src/pax.rs`), Chip (`chip-pax`), and Foundry
(`desktop-agents/src/main/compute/pax.ts`). Compute, Chip, and Foundry were read
and, for Compute's PAX tests, run; none were modified. Every claim below was
checked against code or by running the binary; claims taken from prose are
marked as such. Companion documents: [PAX_BOUNDARY.md](PAX_BOUNDARY.md),
[PAX_CAPABILITIES.md](PAX_CAPABILITIES.md). Behavior pinned by
`tests/boundary.rs` (`known_limitation_*` tests are tripwires for gaps recorded
here).

v0.5.0 update: the canonical schema/outcome contract in
[PAX_CONTRACT.md](PAX_CONTRACT.md) and its documented corrections postdate this
v0.3.0 audit. Statements below about missing schema policies, dry-run plans
having no schema, and an empty project matching in `drift` describe the v0.3.0
baseline, not current behavior.

## Executive conclusion

PAX today is a file-reading project inspector with a thin delegation layer. It
detects JavaScript, Python, Rust, and container projects from manifests and
lockfiles, reports declared dependencies and scripts (accurately for
`package.json`, Cargo, Python TOML/requirements/Pipfile, and Compose services),
parses recognized lockfile package sets, enumerates package names in
conventional install directories, selects a native tool deterministically, and
can run that tool and, for Cargo tests, state what the result means. It does not
resolve dependencies, probe host toolchains, run workloads on its own authority,
or know anything about Node, Python, or Rust runtime versions. Its `reality`
vocabulary describes layered evidence, not verified reality. `--live` now
performs a bounded, read-only Docker/Compose observation; it reports absence
separately from unavailable or failed runtime evidence.

## What PAX actually does

Inventory (all verified in code; "Det." = deterministic given identical files):

| Capability | Implemented | Evidence (code) | Output | Det. | External dependency | Tested |
| --- | --- | --- | --- | --- | --- | --- |
| Project discovery | yes | `collect_project_dirs`, `detect_components` (recurses dirs not starting with `.`, skipping `target`, `node_modules`) | `components[]` | yes | none | `info_json_detects_python_rust_and_docker_components` |
| JS manifest parse | yes | `read_package_json` (serde_json; errors are explicit, exit 1) | `deps`, `scripts`, `workspaces` | yes | none | several |
| JS manager selection | yes | `select_manager`, `manager_from_lockfile` | `manager`, `evidence.selectionNotes` | yes | none | `package_manager_field_wins…`, `lockfile_precedence…` |
| Python detection/parse | yes (pipenv: discovery only) | `detect_python_component` parses TOML (`[project]`, optional groups, `[dependency-groups]`, Poetry, `[build-system].requires`), standalone and nested requirements files, and Pipfile | `components[]`, `nativeDependencies[]` | yes | none | `tests/discovery.rs` |
| Rust via Cargo | yes | `detect_cargo`: `cargo metadata --no-deps --offline`; else TOML fallback with `source: "Cargo.toml fallback"` | `cargo{}`, `nativeDependencies[]` | yes | `cargo` on PATH | `cargo_workspace_*` tests |
| Container detection/parse | yes | `detect_container_component`; Dockerfile line scanner, Compose services limited to direct children of `services:` | `container{}` | yes | none | `tests/discovery.rs` |
| Dependency graph | yes (declared edges only) | `build_graph` | `nodes`, `edges`, `evidence` | yes (sorted) | none | `graph_json_preserves…` |
| "Reality" layers | partial | `build_reality` (field table below); `src/lockfile.rs` parses recognized lockfiles | 4 layers + `dependencies[]` | yes | none | `reality_json_is_static…`, `tests/reconciliation.rs` |
| Drift | partial | `build_drift`; reconciles parsed lockfile package sets and installed package names | `status`, `issues[]` | yes | none | `drift_json_reports_conflicts…`, `malformed_lockfile_is_unknown…` |
| `--live` | yes (Docker/Compose only) | `inspect_runtime`: `docker info`, `docker compose -f … ps --format json`, bounded by a 5s timeout; reports scope, timestamp, permission, commands, failures | `runtime{}` in `reality`/`drift` | no (clock, daemon) | `docker` on `PATH` and a reachable daemon | `tests/runtime.rs` |
| Host tool/runtime detection | not implemented | `detect_node_runtime()` is `fn … { None }` | `result.runtime: null` | — | — | pinned |
| Tool selection for operations | yes | `resolve_operation`, `selection_ambiguity` (fail-closed) | plan / exit 2 | yes | none | many |
| Dry-run plan | yes | `execution_plan` | plan JSON (no schema field) | yes | none | several |
| Native delegation | yes | `dispatch_execution`, `Command::new(program)` | native stdio, exit code | no (native) | native tool | several |
| Test result interpretation | yes (Cargo; JavaScript for Vitest, Jest and `node --test` scripts) | `execution_result.rs` | `pax.execution-result.v1` | yes given same native result | `cargo` | 12 integration + 7 unit |
| Timestamps / observer identity | runtime only | `--live` records `runtime.observedAt` (RFC 3339 UTC); static observation has no clock-derived fields | `runtime.observedAt` | — | — | `tests/runtime.rs`, `structured_output_is_versioned…` |

Process execution in the whole crate: `Command::new` is used for `cargo
metadata` (inspection), the bounded `--live` Docker runner, `dispatch_execution`
(delegation), and the execution-result runner. Static observation reads no
environment variables; `--live` reads `PATH` to locate `docker`, and
`env::current_dir` is used for project resolution. There is no network code.

## What PAX observes

Static observation, directly at invocation time, with no timestamp recorded:

- existence of: `package.json`, `package-lock.json`, `pnpm-lock.yaml`,
  `yarn.lock`, `bun.lock(b)`, `pnpm-workspace.yaml`, `pyproject.toml`,
  `requirements*.txt`, `Pipfile`, `uv.lock`/`poetry.lock`/`pdm.lock`/
  `Pipfile.lock`, `Cargo.toml`, `Cargo.lock`, `Dockerfile*`, compose files;
- existence of directories: `node_modules`, `target`, `.venv`/`venv`/`.env`;
- contents of `package.json` (name, `packageManager`, dependency maps, scripts,
  workspaces) for every `package.json` in the project, including nested ones;
- TOML of `pyproject.toml`, `Pipfile`, and `Cargo.toml` (fallback), requirement
  lines of `requirements*.txt` and `requirements/*.txt`, Dockerfiles, and
  compose files (direct children of `services:`);
- Cargo's own metadata (packages, members, declared dependencies, lockfile
  path) via `cargo metadata`.

PAX does **not** observe: which tools are installed or their versions or paths;
Node/Python/Rust runtime versions; lockfile contents beyond the recognized
formats it parses; installed package versions; environment variables;
OS/arch; running containers; network; git state. It enumerates installed
package *names* from `node_modules/` and Python `site-packages/`, but never
executes a package manager or imports a package.

## What PAX derives

- `ecosystem` (single value only if every component agrees, else `null`).
- Package-manager selection and the reason (`packageManager` field, then
  lockfile precedence; Python: first of uv/poetry/pdm lockfile, else `pip`).
- Graph nodes/edges (project → declared dependency; workspace → member).
- `drift` issues (declared dependency vs. parsed lockfile package set; multiple
  lockfiles; declared dependency vs. enumerated installed packages; missing
  `node_modules`; uninterpretable lockfiles as `unknown`).
- Execution plans (tool, command vector, evidence, selection reason).
- Execution results (`status`, `reason`, `tests`) from native output.

## What PAX claims

Claims in output and what supports them:

- *"X is present"* — a file or directory exists. Supported.
- *"Detected npm package reality for j"* (`info.result.summary`) — overstated: a
  manager was selected from a field or lockfile; nothing about reality was
  verified.
- *"installed: present"* — at least one package name was enumerated in
  `node_modules/` or Python `site-packages/` (empty `node_modules/` is
  `absent`; Rust's `target/` is build output, not installed dependencies).
- *"resolved: present"* — a recognized lockfile was read and its package set
  parsed. `unknown` means the file exists but could not be interpreted.
- *"drift: declared dependency missing from resolvable lockfile"* — the
  dependency is absent from the parsed package set of an established lockfile.
  Unknown or unparseable lockfiles produce `unknown`, not `drift`.
- *"drift: declared dependency installed"* — the dependency name was not found
  in an established installed-package set. Peer JavaScript dependencies are
  excluded from lockfile reconciliation.
- *"tests passed/failed/not_run"* — established from Cargo's exit status and
  libtest summaries, with reasons. Supported and tested.
- *"unsupported"/"ambiguous"* — mirrors fail-closed selection. Supported.

## What PAX does not do

See "PAX does not" in [PAX_BOUNDARY.md](PAX_BOUNDARY.md). In short: no
dependency resolution, installation, environment creation, deployment, runtime
management, provenance, security assessment, application or business
semantics, or reasoning. The one caveat is that PAX *delegates* to tools that do
these things.

## The "reality" model, field by field

Layered semantics for `pax --json reality` (`build_reality`). Static fields are
project-local or machine-local filesystem state at invocation and carry no
timestamp, so staleness is unbounded; evidence is a list of relative paths.
The `runtime` layer is the exception: it is only populated under `--live`, and
then reports its own scope, RFC 3339 UTC observation time, permission,
attempted commands, and failures.

| Field | Meaning | Source | Obs./Deriv. | Evidence | Staleness | If it says true/present, PAX has proven |
| --- | --- | --- | --- | --- | --- | --- |
| `schema_version` | contract version | constant `"1"` | constant | — | — | nothing about the project |
| `command` | `"reality"` | constant | constant | — | — | — |
| `live` | `--live` was passed | CLI flag | observation of args | — | — | nothing about runtime (see `runtime`) |
| `declared.observations[].status` | always `present` for each detected component | filesystem | observation | manifest paths | until files change | a manifest file exists; not that it parses or declares anything |
| `declared…subject` | component path (`.` or subdir) | filesystem | derivation | — | — | — |
| `declared…source` | always `filesystem` | constant | constant | — | — | — |
| `resolved.observations[].status` | one per lockfile: `present` when the recognized format parsed, `unknown` when structurally empty/unparseable/unrecognized, `failed` on read error | filesystem + parsed contents (`src/lockfile.rs`) | observation | lockfile path; `format`; `packageCount` when established; `detail` when not | until files change | the recorded dependency set was established. Not that it matches the manifest, is complete, or is installed |
| `installed.observations[].status` | JS: `present`/`absent` from enumerated package names in `node_modules/`; Rust/Container: always `unknown`; Python: `present`/`absent` when `site-packages/` was enumerated, `unknown` when no conventional environment exists | filesystem | observation | directory name + `packageCount` | until files change | package names exist in that directory. Not versions, completeness, or lockfile agreement |
| `dependencies[]` | one entry per declaration with separate `declared`, `lockfile`, `installed`, and optional `runtime` facts | manifest + lockfile + installed directories | observation/derivation | each fact's `source`/`evidence` | until files change | each layer independently; no layer implies another |
| `runtime` | `{ "observations": [] }` unless `--live`. With `--live`: `scope`, `observedAt` (RFC 3339 UTC), `permission: "authorized (--live)"`, attempted `commands`, optional `failure`, and one observation per declared Compose service | bounded `docker info --format {{.ServerVersion}}` and `docker compose -f <file> ps --format json` (5s timeout each), plus the parsed Compose file | observation (external tool) | Compose file path; per-observation `scope` and optional `failure` | the observation time is recorded; state is stale immediately after | `present` means Compose reported the service running; `absent` means the Compose inspection succeeded and the service was not running. `blocked`, `failed`, `unsupported`, and `unknown` establish nothing |
| `cargo` | Cargo packages, members, dependencies, lockfile | `cargo metadata --no-deps --offline` or heuristic fallback | observation (external tool) / heuristic | `source` field names the method | until files change | Cargo reported these packages. `--no-deps` means dependency *resolution* is not observed |

`drift` (`build_drift`): `issues[].status` is `drift`, `ambiguous`, or
`unknown`; `expected`/`actual` are human strings; `evidence[].kind` is
`declared`, `resolved`, `installed`, or `runtime`. Top-level `status`
precedence is `drift` > `ambiguous` > `unknown` > `match` (`unsupported` when
nothing was detected). Exit codes are `1` for `drift`, `2` for `ambiguous`, and
`0` for `match`, `unknown`, and `unsupported`; text mode prints `UNKNOWN` or
`AMBIGUOUS` so evidence gaps are not confused with confirmed contradictions.
`drift --live` adds a `runtime` layer with the same shape as `reality`; runtime
`present`/`absent` are established and do not by themselves create an issue,
while `blocked`/`failed`/`unsupported`/`unknown` produce an `unknown` issue.
A JS component without `node_modules/` reports `drift` only when it declares at
least one runtime dependency.

`graph`: nodes and edges are declared dependencies and workspace membership;
`evidence` is the manifest path. It is a declared graph, not a resolved one.

### Project model fields (`info`, `deps`, `scripts`, `lock`, `workspaces`)

| Field | Source | Authority | Semantics | Stale? | Evidence | Scope |
| --- | --- | --- | --- | --- | --- | --- |
| `project.name` | package.json `name`, else Cargo package, else **directory name** | project / PAX fallback | declared or inferred (not distinguished) | yes | no | project-local |
| `project.root` | CLI `--dir`/cwd | host | observed; **absolute machine path** | yes | no | machine-local |
| `ecosystem` | component ecosystems | PAX | derived | yes | via `components` | project-local |
| `manager.name/version/lockfile/selectedBy` | `packageManager` field / lockfile | project | declared (field) or inferred (lockfile) | yes | `evidence.*` | project-local |
| `components[]` | manifests, lockfiles | filesystem | detected | yes | `evidence[]` | project-local |
| `dependencies.*` (JS) | package.json at the project root | project | declared | yes | `source` on native dependencies; root groups are implicit | project-local |
| `nativeDependencies[]` | Cargo metadata / Python TOML, requirements, Pipfile / fallback Cargo / nested package.json | project | declared; each item names its `source` manifest or requirements file | yes | `source` | project-local |
| `scripts` | package.json | project | declared | yes | no | project-local |
| `container.*` | Dockerfile/compose files | project | declared from direct `services:` children; Dockerfile directives are line-scanned | yes | no | project-local |
| `diagnostics[]` | PAX checks | PAX | derived | yes | message only | project-local |
| `result.runtime` | `detect_node_runtime()` | — | always `null` | — | — | — |

## Parser corrections in 0.5.0

The v0.3.0 findings below were fixed and are pinned by `tests/discovery.rs`:

1. **Python `pyproject.toml`** is parsed as TOML. `[project].dependencies`,
   `[project.optional-dependencies]`, `[dependency-groups]`,
   `[tool.poetry]` dependencies/groups, and `[build-system].requires` are read;
   project metadata such as `name` and `version` is not a dependency.
2. **`requirements*.txt`** is discovered independently of `pyproject.toml`,
   including top-level variants and `requirements/*.txt`. Requirement lines,
   extras, environment markers, comments, and line continuations are
   interpreted; pip options are skipped and URL requirements are reported as
   uninterpreted.
3. **`Pipfile`** declarations are read from `[packages]` and `[dev-packages]`,
   and the project tool is reported as `pipenv`. Pipenv execution is
   unsupported and fails with an explicit error.
4. **Compose** services are limited to direct children of `services:`;
   `ports`, `environment`, `build`, and other nested keys are not services.
   Images are collected from service definitions.
5. **Malformed manifests** produce Error diagnostics instead of silent or
   invented observations: broken `pyproject.toml`, `Pipfile`, `Cargo.toml`, and
   nested `package.json` files make the inspection outcome `partial`. A broken
   root `package.json` remains an explicit exit-1 error. Malformed requirement
   lines produce Warning diagnostics.
6. **Diagnostics are no longer JavaScript-centric**: JavaScript-specific
   checks are emitted only when `package.json` exists; Python-only projects
   report their own manifest and lockfile diagnostics.
7. **Ambiguous Python manifests**: a project with both `pyproject.toml` and
   `Pipfile` reports a Warning and interprets `pyproject.toml`.

Remaining limitations: declared runtime requirements (`engines.node`,
`requires-python`, `rust-version`, `.nvmrc`, `.tool-versions`,
`rust-toolchain`) are still not read, and pipenv cannot be executed.

## JSON contract audit

| Question | Finding |
| --- | --- |
| Schema version | `schemaVersion: "1"` in `info`-family documents (camelCase); `schema_version: "1"` in `graph`/`reality`/`drift` (snake_case); `pax.execution-result.v1` in execution results; dry-run plans have **no version field**. |
| Compatibility guarantee | None written for observation documents. Nothing prevents fields being added, or values changing, under the same `"1"`. A semantic correction (for example fixing the Python parser) would change values under the same `"1"`. The execution result documents a rejection policy; the others do not. |
| Required / optional fields | Not specified. `skip_serializing_if` omits `dependencies`, `scripts`, `workspaces`, `lock`, `cargo` by command; other `Option`s serialize as `null`. |
| Unknown fields | Consumers (Compute) ignore them; nothing tells consumers they may appear. |
| Error representation | Observation commands: human message on stderr, exit 1 or 2, empty stdout. Only `--json test` returns a structured error. |
| Provenance | File lists in `components`, `graph`, `reality`, `drift`; native dependencies name their manifest/requirements `source`; diagnostics name a check and message. See [PAX_CONTRACT.md](PAX_CONTRACT.md) for canonical evidence. |
| Timestamps | None, anywhere. Deterministic: verified by running each command twice (`structured_output_is_versioned_and_deterministic`). |
| Source information | `cargo.source` names the Cargo method; native dependencies name their declaring manifest or requirements file; `components[].evidence[]` and graph/reality/drift evidence name files. |
| Ordering | Graph, drift, dependencies, components are sorted; JSON object key order follows Rust struct order. |
| Stable identifiers | None. `project.name` can be a directory name; `project.root` is machine-specific. |

Consumers may rely on: the presence and types of fields in
`consumer_contract_for_compute_adapter`; determinism for fixed files and a fixed
Cargo; the execution-result contract in README. They may not rely on semantic
stability of `reality`/`drift` values, on `schemaVersion` implying
compatibility, or on any field being a verified fact.

## Consumers

| Consumer | PAX interface | Notes |
| --- | --- | --- |
| **Compute** (`compute-project/src/pax.rs`) | `pax --json --dir <root> info`, `deps`, `scripts` | normalization below |
| **Chip** (`chip-rs/crates/chip-pax`) | `pax --dir <d> --json test` → `pax.execution-result.v1` | parses stdout strictly; requires PAX ≥ 0.3.0; cites `status` and `exit_code` separately (described in its own header; not audited further) |
| **Foundry** (`desktop-agents/src/main/compute/pax.ts`) | all inspection commands incl. `reality`, `drift`; `build/test/lint/typecheck/install`, `--dry-run` | passes JSON through unmodified to users (per its header comment), so the overclaims above reach people |

### Compute: PAX output → requirements

```text
pax info/deps/scripts ──▶ PaxObservation::from_documents (validate)
                          ──▶ requirements() ──▶ ProjectRequirements (normalize)
                          ──▶ planning/placement ──▶ execution
```

| PAX field read | Transformation | Compute assumes | Discarded |
| --- | --- | --- | --- |
| `schemaVersion`, `command` | must equal `"1"` / expected command | `"1"` means the understood shape | — |
| `project.name` | project identity; empty ⇒ invalid | the name identifies the project (it can be the directory name, so the empty check never fires) | `project.root`, `workspace`, `workspaceSource` |
| `components` | non-empty ⇒ is a project; first match supplies tool fallback | any component means a project | components' manifests, lockfiles, evidence, nested components |
| `ecosystem` | `javascript`→Node, `python`→Python, anything else (Rust, container, mixed `null`) ⇒ `unresolved` | single ecosystem | — |
| `manager.name/version` | provisioning tool (never run by Compute) | declared, not verified on target | `selectedBy`, `lockfile`, `evidence`, `selectionNotes` |
| `dependencies.{dependencies,devDependencies,optionalDependencies,peerDependencies}` | dependency needs | declared specifiers | — |
| `dependencies.nativeDependencies` | non-empty ⇒ `unresolved` | PAX reports non-JavaScript declarations here, each with its `source` | content (Compute does not normalize native declarations) |
| `scripts.scripts` | commands | declared | — |

What Compute discards that matters:

- **Workspaces.** `project.workspace`/`workspaceSource` and the `workspaces`
  document are not read. A multi-package JS workspace runs as the single root
  project. Compute's docs list this under "Not yet supported", but the code does
  not refuse it (no `workspace` reference exists in `compute-project/src`).
- **PAX diagnostics**, including the `package/lock consistency` warning and
  ambiguity notes. `info` succeeds with conflicting lockfiles (selection by
  precedence); only operation commands fail closed on ambiguity.
- **Runtime requirements**: PAX never reports them, so Compute's
  "runtime version" is always user-supplied; the docs say so.

**The rule "anything PAX declares that Compute cannot normalize must not
silently disappear" is guaranteed only for two classes**: an ecosystem other
than JavaScript/Python, and a non-empty `nativeDependencies`
(`what_compute_cannot_normalize_is_never_dropped`). It is not guaranteed for
workspaces, diagnostics, lockfile conflicts, or unknown fields. Compute's
refusal of Python today is protective in part because PAX's Python output is
wrong; correcting PAX's parser without changing Compute would change behavior.
Compute's PAX tests (21) pass against the current adapter.

## Declared ≠ observed ≠ resolved ≠ verified

- *Declared requirement*: PAX reports dependencies and scripts, not runtime or
  platform requirements.
- *Observed host capability*: PAX reports none (no PATH lookup, no versions).
  For "requires Node 24, host has Node 22", PAX observes neither side.
- *Resolved requirement*: PAX reports lockfile existence; Cargo metadata is
  invoked with `--no-deps`, so resolution is not observed.
- *Verified execution*: only `--json test` for Cargo, as a statement about the
  native tool's result.

The architecture keeps these apart: `reality` has separate layers, drift
evidence is labelled, and Compute's receipts carry `declared`/`resolved`/
`verified`/`actual` blocks. Dependency evidence is now separated per layer:
`declared`, lockfile-recorded, installed, and runtime facts each carry their
own source and state. `info`/`deps` still mix declared and inferred values
without marking which is which.

## Documentation overclaims

| Where | Claim | Reality |
| --- | --- | --- |
| README line 3 | "the universal, read-only project-tooling boundary" | four ecosystems; not read-only as a whole (README line 8 concedes delegation) |
| README | "`pax info` reports detected package-manager reality" | reports selection from a field/lockfile |
| README | "runtime inspection is opt-in with `--live`" | Fixed: `--live` performs a bounded, read-only Docker/Compose observation of detected Compose services and reports scope, time, permission, commands, and failures |
| README | "`pax reality` separates declared, resolved, installed, and runtime observations" | Fixed: each dependency layer carries its own state, source, and evidence; the runtime layer reports observed service state or an explicit unavailable/unsupported/failed state |
| README | "Cargo projects use … `cargo metadata --no-deps` for authoritative workspace semantics" | true when cargo succeeds; TOML fallback otherwise (provenance names the fallback) |
| README | "native dependency semantics, and static container services" | accurate for Python TOML, requirements, Pipfile, nested `package.json`, and direct Compose services; unknown constructs are diagnosed |
| README | "`pax drift` reports contradictions between those layers" | Fixed: reconciles parsed lockfile package sets and enumerated installed packages; unknown evidence is not a contradiction |
| README | "unknown evidence is never promoted to drift" | Fixed: unknown evidence keeps top-level `status: "unknown"` and exit code 0 |
| `info.result.summary` | "Detected … package reality" | string constant, see above |
| `doctor` | `result.runtime` | always `null` |
| Compute `docs/pax.md` | "PAX defines project and environment requirements"; "PAX … Owns what the project requires" | PAX reports declared dependencies and scripts only; no runtime, platform, or tool requirements |
| Compute `docs/pax.md` | "Multi-package workspaces run as the single project" under unsupported | code does not refuse |
| Compute docs | "versioned JSON (`schemaVersion: "1"`)" | no compatibility policy behind the version |

## Defects reported by a consumer (Chip adapter, post-0.3.0)

| # | Defect | Status |
| --- | --- | --- |
| 1 | `pax graph` attributed the workspace-wide Cargo dependency union to every member, with evidence pointing at a manifest that did not declare the edge (including self-edges). | Fixed after 0.4.0: Rust component edges come from `cargo metadata`'s per-package dependencies (`tests/cli.rs::graph_attributes_cargo_dependencies_to_the_declaring_package_only`). `info`/`deps` still report only the union (`nativeDependencies` has no package attribution); when `cargo metadata` is unavailable, `graph` falls back to the union. |
| 2 | `observe` following symlinks out of the project. | The reported repro does not escape: both links resolve inside the project root. `file:` and `path:` scopes already refuse targets whose canonical path is outside the root. A real gap was found while checking: `mod x;` resolution could read a symlinked module file outside the root. It now yields an `artifact_outside_root` diagnostic and is not read. |
| 3 | Version unchanged from 0.3.0 while `observe` was added. | Fixed in 0.4.0. |
| 4 | `graph` mixes identifier types (absolute path for `workspace-member` edges, relative dirs, package names). | **Open.** Changing `graph` identifiers changes existing output, so it needs a versioned `graph` schema. |
| 5 | `target/` and `target-linux/` tracked in git. | Untracked and ignored. |
| 6 | `graph` has no scoping or bound; output is the whole workspace. | **Open.** Use `observe` for bounded questions. |
| 7 | `observe` input inconsistencies (`path:` and `--max-files 0`). | Fixed: empty scope values and non-positive limits are `invalid_scope` / `invalid_limit`; `path:.` means the whole project. |

Until #4 and #6 are addressed, treat `graph` as a declared-relationship summary
whose identifiers must not be joined across edge kinds without inspecting them.

## Product gaps (PAX-owned, prioritized)

1. **P0** ~~Stop `--live` and `runtime` from implying observation (remove the
   claim, or implement a real observation with a recorded command and result).~~
   Fixed in 0.5.0: `--live` runs bounded `docker info` and `docker compose ps`,
   and reports scope, timestamp, permission, attempted commands, and failures.
   Missing CLI, inaccessible daemon, command failure, and absence are distinct
   states (`tests/runtime.rs`).
2. **P0** ~~Fix or fence the Python parser~~ Fixed in 0.5.0: real TOML parse,
   independent `requirements*.txt` discovery, Pipfile declarations, and
   explicit `partial` outcomes for malformed manifests.
3. **P1** ~~Add explicit unparseable/unknown diagnostics for every manifest PAX
   reads; remove JavaScript-only diagnostics from non-JS projects.~~ Fixed in
   0.5.0 (`detection_diagnostics`; JS checks only when `package.json` exists).
4. **P1** Report declared runtime requirements as declared observations
   (`engines`, `requires-python`, `rust-version`, `.nvmrc`), with source file.
5. **P1** One schema/versioning convention and a written compatibility policy
   for observation documents; self-describing identifiers like
   `pax.execution-result.v1`.
6. **P1** Per-field provenance (file + key) and an explicit `declared` /
   `inferred` / `observed` marker.
7. **P1** ~~Rename or restate the `installed`/`resolved` layers as presence
   observations; stop counting empty directories and `target/`.~~ Fixed:
   `resolved` reports parsed lockfile state with format and package count;
   empty `node_modules/` is `absent`; Rust installed state is always `unknown`
   and `target/` is no longer evidence.
8. **P2** Tool detection (path and version, observation only) if a consumer needs
   it; Compose and Dockerfile parsing; a stable project identity without
   machine paths; optional, separate observation timestamp.
9. **P3** More ecosystems, more test-result interpreters, only with concrete
   consumers.

## Architectural gaps

1. PAX has two distinct jobs (observation; delegation) in one binary and one
   file. The boundary is clear in commands but not in the documentation.
2. `deploy`, `install`, `add`, `remove`, `exec` push PAX toward being an
   execution system. No code is moved in this PR; whether they stay is a
   product decision (see below).
3. Consumers can read `reality`/`drift` (Foundry does). The layered dependency
   evidence and unknown status are documented in PAX_CONTRACT.md; the audit
   keeps them separate from verified reality.
4. Compute's "never dropped" rule has no mechanism, only two checks (above).
5. No contract test across repositories: PAX pins the fields Compute reads
   (`consumer_contract_for_compute_adapter`), Compute pins its own fixtures; a
   change in either is not caught by the other's suite.

## Recommended next steps

**Do now**

- Land this audit and the README corrections.
- ~~Fix P0 items (they are bugs, not features): `--live` claim, Python parser.~~
  Both P0 items are fixed in 0.5.0.
- Add a compatibility policy for `schemaVersion: "1"` before changing values.
- Compute (separate PR): refuse JS workspaces explicitly, or document precisely
  that they are not honored.

**Do later**

- Unified schema convention; declared runtime requirements; provenance fields;
  explicit unknown/unparseable states; tool detection if needed.
- Decide whether `deploy` and package mutation belong in PAX.

**Don't do**

- Dependency resolution, lockfile satisfiability, installation, environment or
  toolchain management, sandboxing, persistent state, provenance signing,
  vulnerability analysis, goal evaluation, retries/repair, LLM interpretation
  (see [PAX_BOUNDARY.md](PAX_BOUNDARY.md)).
