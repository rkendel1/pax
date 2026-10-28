# Changelog

## 0.5.0

- add the canonical observation contract (`pax.contract.v1`): versioned
  `schema` identifiers, typed `outcome` (`ok`, `partial`, `unsupported`,
  `blocked`, `failed`, `stale`), distinct fact states, traceable evidence,
  project-relative entity identities, and a compatibility policy; see
  `docs/PAX_CONTRACT.md` and `src/contract.rs`
- every inspection JSON document now carries `schema` and `outcome`; the legacy
  `schemaVersion` / `schema_version` field remains `"1"` for compatibility
- `pax observe` keeps `status` and adds `outcome`; unsupported-only observations
  report `unsupported`, never `ok`
- dry-run plans now declare `schema: "pax.plan.v1"` (documented correction)
- graph identities are project-relative: the project root is `.`, workspace
  roots are never absolute paths, and paths outside the root are opaque
- a single-package Cargo project is no longer reported as a workspace by
  `workspaces` or `graph`
- `drift` reports `status`/`outcome` `unsupported` when there is nothing to
  observe instead of claiming `match`
- fix command-word parsing so `pax run drift` and `--dir drift` do not select
  drift exit-code semantics
- add contract fixtures and `tests/contract.rs`; update README and observation
  docs
- parse Python declarations from real TOML (`[project]`, optional dependency
  groups, `[dependency-groups]`, Poetry sections, `[build-system].requires`),
  standalone `requirements*.txt`, `requirements/*.txt`, and `Pipfile`; every
  native dependency now carries a project-relative `source`
- detect Pipfile projects with `tool: "pipenv"`; pipenv operations fail with an
  explicit unsupported error
- discover every `package.json` as a component, add nested declarations to
  `nativeDependencies` with provenance, attribute graph edges to the declaring
  component, and accept hoisted `node_modules/` for nested packages
- parse `Cargo.toml` for workspace detection and members, including trailing
  comments, instead of relying on line equality
- limit Compose services to direct children of `services:` and deduplicate them
- report malformed Python, Cargo, Pipfile, and nested JavaScript manifests as
  Error diagnostics and `outcome: "partial"`; malformed requirement lines are
  Warnings. JavaScript-specific diagnostics are emitted only for JavaScript
  projects
- `pax deps` no longer reports `partial` merely because Python is present;
  `pax lock` includes and selects Python component lockfiles
- add `tests/fixtures/discovery/` and `tests/discovery.rs` covering valid,
  malformed, and ambiguous Python, Cargo, nested JavaScript, and Compose
  projects

## 0.4.1

- fix `pax graph`: Cargo dependency edges are attributed to the declaring
  package from `cargo metadata`, not the workspace-wide union (existing `info`
  and `deps` output unchanged)
- `observe`: refuse to read `mod x;` files through symlinks that leave the
  project root (`artifact_outside_root`); reject empty scope values and
  non-positive limits as `invalid_scope` / `invalid_limit`
- record consumer-reported defects in `docs/PAX_AUDIT.md`; stop tracking
  `target/` and `target-linux/`

## 0.4.0

- add `pax observe` (`pax.observation.v1`): bounded, deterministic project-structure
  observation (artifacts, workspace members, declared dependencies, and Rust
  crate/module/file/declaration structure) with per-fact provenance, explicit
  limits, and typed errors; see `docs/PAX_OBSERVATION.md`. Additive: existing
  commands and output are unchanged. Adds `syn` and `proc-macro2` dependencies
- add `docs/PAX_AUDIT.md`, `docs/PAX_BOUNDARY.md`, and `docs/PAX_CAPABILITIES.md`
- correct README claims that overstated `reality`, `--live`, and read-only scope
- add `tests/boundary.rs`: boundary tests and tripwires for documented gaps
  (no behavior change)

## 0.3.0

- add the `pax.execution-result.v1` contract: `pax --json test` now emits a
  versioned execution result (status, reason, native exit code, optional Cargo
  test counts) on stdout, with native output moved to stderr; `--dry-run --json`
  plans and non-JSON output are unchanged

## 0.2.0

- unify Cargo manifest, lockfile, workspace, package, and dependency observation
  across `info`, `doctor`, `deps`, `workspaces`, `graph`, `reality`, and `drift`
- use Cargo metadata as the authority for resolved workspace membership while
  preserving read-only behavior for workspaces without a lockfile
- report Cargo consistently in human-readable and JSON output
- add regression coverage for multi-crate, lockless, single-package, and virtual
  Cargo workspaces
- publish installable archives and checksums for macOS, Linux, and Windows

## 0.1.0

Initial release of PAX:

- deterministic JavaScript, Python, Rust, container, and deployment detection
- read-only inspection and observation commands with stable JSON output
- fail-closed ambiguity handling and explicit `--tool` overrides
- native-tool delegation for tasks, package execution, installation, mutation,
  and deployment
- `--dry-run`, `--dir`, `--live`, and `--json` controls
