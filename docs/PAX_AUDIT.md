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

## Executive conclusion

PAX today is a file-reading project inspector with a thin delegation layer. It
detects JavaScript, Python, Rust, and container projects from manifests and
lockfiles, reports declared dependencies and scripts (accurately for
`package.json` and Cargo, unreliably for Python and Compose), checks whether a
few conventional directories exist, selects a native tool deterministically, and
can run that tool and, for Cargo tests, state what the result means. It does
not resolve dependencies, probe the host, run workloads on its own authority,
or know anything about runtime versions. Its `reality` vocabulary describes
presence checks, not verified reality, and two behaviors (`--live`, the
`runtime` field) claim more than they do.

## What PAX actually does

Inventory (all verified in code; "Det." = deterministic given identical files):

| Capability | Implemented | Evidence (code) | Output | Det. | External dependency | Tested |
| --- | --- | --- | --- | --- | --- | --- |
| Project discovery | yes | `collect_project_dirs`, `detect_components` (recurses dirs not starting with `.`, skipping `target`, `node_modules`) | `components[]` | yes | none | `info_json_detects_python_rust_and_docker_components` |
| JS manifest parse | yes | `read_package_json` (serde_json; errors are explicit, exit 1) | `deps`, `scripts`, `workspaces` | yes | none | several |
| JS manager selection | yes | `select_manager`, `manager_from_lockfile` | `manager`, `evidence.selectionNotes` | yes | none | `package_manager_field_wins…`, `lockfile_precedence…` |
| Python detection/parse | partial | `detect_python_component`: line scanner, not a TOML parser | `components[]`, `nativeDependencies[]` | yes | none | detection only; parser is wrong (below) |
| Rust via Cargo | yes | `detect_cargo`: `cargo metadata --no-deps --offline`; else line-scan fallback with `source: "Cargo.toml fallback"` | `cargo{}`, `nativeDependencies[]` | yes | `cargo` on PATH | `cargo_workspace_*` tests |
| Container detection/parse | partial | `detect_container_component`: line scanner | `container{}` | yes | none | detection only |
| Dependency graph | yes (declared edges only) | `build_graph` | `nodes`, `edges`, `evidence` | yes (sorted) | none | `graph_json_preserves…` |
| "Reality" layers | partial | `build_reality` (field table below) | 4 layers | yes | none | `reality_json_is_static…` |
| Drift | partial | `build_drift`; lockfile check is `contents.contains(name)` | `status`, `issues[]` | yes | none | `drift_json_reports_conflicts…` |
| `--live` | not implemented | no process is started; `runtime` entries carry `status: "unknown"` | see below | yes | none | `known_limitation_live_*` |
| Host tool/runtime detection | not implemented | `detect_node_runtime()` is `fn … { None }` | `result.runtime: null` | — | — | pinned |
| Tool selection for operations | yes | `resolve_operation`, `selection_ambiguity` (fail-closed) | plan / exit 2 | yes | none | many |
| Dry-run plan | yes | `execution_plan` | plan JSON (no schema field) | yes | none | several |
| Native delegation | yes | `dispatch_execution`, `Command::new(program)` | native stdio, exit code | no (native) | native tool | several |
| Test result interpretation | yes (Cargo only) | `execution_result.rs` | `pax.execution-result.v1` | yes given same native result | `cargo` | 12 integration + 7 unit |
| Timestamps / observer identity | not implemented | no clock use outside tests | — | — | — | `structured_output_is_versioned…` |

Process execution in the whole crate: `Command::new` appears three times —
`cargo metadata` (inspection), `dispatch_execution` and `run_native`
(delegation). Environment variables are never read (`env::var` is absent;
`env::current_dir` is the only environment access). There is no network code.

## What PAX observes

Directly, at invocation time, with no timestamp recorded:

- existence of: `package.json`, `package-lock.json`, `pnpm-lock.yaml`,
  `yarn.lock`, `bun.lock(b)`, `pnpm-workspace.yaml`, `pyproject.toml`,
  `requirements*.txt`, `Pipfile`, `uv.lock`/`poetry.lock`/`pdm.lock`/
  `Pipfile.lock`, `Cargo.toml`, `Cargo.lock`, `Dockerfile*`, compose files;
- existence of directories: `node_modules`, `target`, `.venv`/`venv`/`.env`;
- contents of `package.json` (name, `packageManager`, dependency maps, scripts,
  workspaces);
- text lines of `pyproject.toml`, `requirements*.txt`, `Cargo.toml` (fallback),
  Dockerfiles, compose files (heuristic);
- Cargo's own metadata (packages, members, declared dependencies, lockfile
  path) via `cargo metadata`.

PAX does **not** observe: which tools are installed or their versions or paths;
Node/Python/Rust runtime versions; lockfile contents (beyond a substring search
in drift); installed package contents or versions; environment variables;
OS/arch; running containers; network; git state.

## What PAX derives

- `ecosystem` (single value only if every component agrees, else `null`).
- Package-manager selection and the reason (`packageManager` field, then
  lockfile precedence; Python: first of uv/poetry/pdm lockfile, else `pip`).
- Graph nodes/edges (project → declared dependency; workspace → member).
- `drift` issues (field vs. lockfile; multiple lockfiles; missing
  `node_modules`).
- Execution plans (tool, command vector, evidence, selection reason).
- Execution results (`status`, `reason`, `tests`) from native output.

## What PAX claims

Claims in output and what supports them:

- *"X is present"* — a file or directory exists. Supported.
- *"Detected npm package reality for j"* (`info.result.summary`) — overstated: a
  manager was selected from a field or lockfile; nothing about reality was
  verified.
- *"installed: present"* — directory exists (empty `node_modules/` qualifies;
  Rust's `target/` is build output, not installed dependencies). Weak.
- *"resolved: present"* — a lockfile exists. PAX does not read it for this layer.
- *"drift: declared dependency missing from lockfile"* — name not found by
  substring search; a short name such as `a` matches almost any lockfile, so
  false negatives are likely. Weak.
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

Layered semantics for `pax --json reality` (`build_reality`). All fields are
project-local or machine-local filesystem state at invocation; none carries a
timestamp, so staleness is unbounded; evidence is a list of relative paths.

| Field | Meaning | Source | Obs./Deriv. | Evidence | Staleness | If it says true/present, PAX has proven |
| --- | --- | --- | --- | --- | --- | --- |
| `schema_version` | contract version | constant `"1"` | constant | — | — | nothing about the project |
| `command` | `"reality"` | constant | constant | — | — | — |
| `live` | `--live` was passed | CLI flag | observation of args | — | — | nothing about runtime (see `runtime`) |
| `declared.observations[].status` | always `present` for each detected component | filesystem | observation | manifest paths | until files change | a manifest file exists; not that it parses or declares anything |
| `declared…subject` | component path (`.` or subdir) | filesystem | derivation | — | — | — |
| `declared…source` | always `filesystem` | constant | constant | — | — | — |
| `resolved.observations[].status` | one per lockfile, always `present` | filesystem | observation | lockfile path | until files change | a lockfile exists. Not that it matches the manifest, parses, or is complete |
| `installed.observations[].status` | JS: `present`/`absent` for `node_modules/`; Rust: `present` if `target/` exists else `unknown`; Python: `present` if `.venv`/`venv`/`.env` is a directory else `unknown`; Container: always `unknown` | filesystem | observation | directory name | until files change | a directory with that name exists. Not that dependencies are installed (empty dir counts), complete, or match the lockfile |
| `runtime.observations[]` | empty unless `--live`; with `--live`, one per Container component, `status: "unknown"`, `source: "docker"`, `evidence: ["docker compose ps"]` | constant | **none: no observation occurs** | names a command PAX did not run | — | nothing. The `evidence` string is a statement of intent, not evidence |
| `cargo` | Cargo packages, members, dependencies, lockfile | `cargo metadata --no-deps --offline` or heuristic fallback | observation (external tool) / heuristic | `source` field names the method | until files change | Cargo reported these packages. `--no-deps` means dependency *resolution* is not observed |

`drift` (`build_drift`): `issues[].status` is `drift`, `ambiguous`, or
`unknown`; `expected`/`actual` are human strings; `evidence[].kind` is
`declared`, `resolved`, `installed`, or `runtime`. Top-level `status` is
`drift` if any issue is `drift`, `match` if there are no issues, otherwise
`ambiguous` — so `unknown` runtime items under `--live` produce an
`ambiguous` result (JSON exit code 2) although nothing is ambiguous; in text
mode the same state exits 1 (anything other than the literal `NO DRIFT`
counts as drift, `main.rs`). Also, a JS component without `node_modules/`
reports `drift` ("expected installed dependencies") even when it declares no
dependencies.

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
| `dependencies.*` (JS) | package.json | project | declared | yes | no per-item path | project-local |
| `nativeDependencies[]` | Cargo metadata (accurate) / line scanners (Python, fallback Cargo) | mixed | declared (Cargo) / **misparsed (Python)** | yes | no | project-local |
| `scripts` | package.json | project | declared | yes | no | project-local |
| `container.*` | Dockerfile/compose lines | project | heuristic | yes | no | project-local |
| `diagnostics[]` | PAX checks | PAX | derived | yes | message only | project-local |
| `result.runtime` | `detect_node_runtime()` | — | always `null` | — | — | — |

## Parsers that do not match their claims (verified by running the binary)

1. **Python `pyproject.toml`**: every `key = "string"` line becomes a
   "dependency". For `name`, `version`, `requires-python`, `dependencies`, PAX
   reports four dependencies named exactly that. The actual dependency
   (`requests>=2`) is not extracted. `requires-python` (a runtime requirement)
   is reported as a dependency.
2. **`requirements.txt` alone**: no dependencies reported. The loop that reads
   requirements files is nested inside the `pyproject.toml` line loop.
3. **Compose**: any `word:` line without spaces becomes a service, so `ports`,
   `environment`, `build` are reported next to `web`.
4. **Malformed manifests**: a broken `pyproject.toml` yields `ecosystem:
   python` and no diagnostic. A broken `Cargo.toml` falls back to a line
   scanner (`source` says so, but diagnostics still say "Cargo.toml found").
   Only an unparsable `package.json` is an explicit error.
5. **Diagnostics are JavaScript-centric**: a Python-only or empty directory
   reports `error: package.json is required to inspect a JavaScript package`.
6. **Declared runtime requirements are ignored**: `engines.node`,
   `requires-python`, `rust-version`, `.nvmrc`, `.tool-versions`,
   `rust-toolchain` are not read, so none appears in any output.

## JSON contract audit

| Question | Finding |
| --- | --- |
| Schema version | `schemaVersion: "1"` in `info`-family documents (camelCase); `schema_version: "1"` in `graph`/`reality`/`drift` (snake_case); `pax.execution-result.v1` in execution results; dry-run plans have **no version field**. |
| Compatibility guarantee | None written for observation documents. Nothing prevents fields being added, or values changing, under the same `"1"`. A semantic correction (for example fixing the Python parser) would change values under the same `"1"`. The execution result documents a rejection policy; the others do not. |
| Required / optional fields | Not specified. `skip_serializing_if` omits `dependencies`, `scripts`, `workspaces`, `lock`, `cargo` by command; other `Option`s serialize as `null`. |
| Unknown fields | Consumers (Compute) ignore them; nothing tells consumers they may appear. |
| Error representation | Observation commands: human message on stderr, exit 1 or 2, empty stdout. Only `--json test` returns a structured error. |
| Provenance | File lists in `components`, `graph`, `reality`, `drift`; absent from `deps`, `scripts`, `manager.version`, diagnostics. |
| Timestamps | None, anywhere. Deterministic: verified by running each command twice (`structured_output_is_versioned_and_deterministic`). |
| Source information | `cargo.source` names the Cargo method; nothing else names its method. |
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
| `dependencies.nativeDependencies` | non-empty ⇒ `unresolved` | PAX reports Python only this way | content (they are not even correct for Python) |
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
`verified`/`actual` blocks. The ambiguity is inside PAX: `reality`'s
`declared` and `resolved` names are presence checks, and `info`/`deps` mix
declared and inferred values without marking which is which.

## Documentation overclaims

| Where | Claim | Reality |
| --- | --- | --- |
| README line 3 | "the universal, read-only project-tooling boundary" | four ecosystems; not read-only as a whole (README line 8 concedes delegation) |
| README | "`pax info` reports detected package-manager reality" | reports selection from a field/lockfile |
| README | "runtime inspection is opt-in with `--live`" | `--live` inspects nothing |
| README | "`pax reality` separates declared, resolved, installed, and runtime observations" | presence checks; runtime layer has no observations |
| README | "Cargo projects use … `cargo metadata --no-deps` for authoritative workspace semantics" | true when cargo succeeds; heuristic fallback otherwise |
| README | "native dependency semantics, and static container services" | Python deps misparsed; Compose services over-reported |
| README | "`pax drift` reports contradictions between those layers" | partly substring checks; `unknown` surfaces as `ambiguous` |
| README | "unknown evidence is never promoted to drift" | true, but promoted to `ambiguous` |
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

1. **P0** Stop `--live` and `runtime` from implying observation (remove the
   claim, or implement a real observation with a recorded command and result).
2. **P0** Fix or fence the Python parser (real TOML parse, or report
   "not interpreted"); read `requirements.txt` independently of
   `pyproject.toml`.
3. **P1** Add explicit unparseable/unknown diagnostics for every manifest PAX
   reads; remove JavaScript-only diagnostics from non-JS projects.
4. **P1** Report declared runtime requirements as declared observations
   (`engines`, `requires-python`, `rust-version`, `.nvmrc`), with source file.
5. **P1** One schema/versioning convention and a written compatibility policy
   for observation documents; self-describing identifiers like
   `pax.execution-result.v1`.
6. **P1** Per-field provenance (file + key) and an explicit `declared` /
   `inferred` / `observed` marker.
7. **P1** Rename or restate the `installed`/`resolved` layers as presence
   observations; stop counting empty directories and `target/`.
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
3. Consumers can read `reality`/`drift` (Foundry does) but nothing tells them
   the layers are presence checks.
4. Compute's "never dropped" rule has no mechanism, only two checks (above).
5. No contract test across repositories: PAX pins the fields Compute reads
   (`consumer_contract_for_compute_adapter`), Compute pins its own fixtures; a
   change in either is not caught by the other's suite.

## Recommended next steps

**Do now**

- Land this audit and the README corrections.
- Fix P0 items (they are bugs, not features): `--live` claim, Python parser.
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
