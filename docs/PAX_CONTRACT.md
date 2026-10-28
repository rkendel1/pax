# PAX observation contract

Status: v0.5.0. Code: [`src/contract.rs`](../src/contract.rs). Tests:
[`tests/contract.rs`](../tests/contract.rs). Fixtures:
[`tests/fixtures/contract/`](../tests/fixtures/contract/).

This document is the compatibility contract for every JSON document PAX emits.
It exists so a consumer can answer four questions without guessing:

1. **What is this?** — the canonical `schema` identifier.
2. **How far did PAX get?** — the typed `outcome`.
3. **What exactly was established?** — a fact state, not a boolean.
4. **Where did it come from?** — evidence naming a source.

PAX observes the project. It does not decide relevance, correctness, plans, or
whether a consumer's goal is satisfied. This contract does not add observers; it
makes the existing ones honest about their limits.

## Schema naming

Every JSON document carries a canonical `schema`:

```text
pax.<subject>.v<major>
```

| Document | `schema` | Legacy field retained |
| --- | --- | --- |
| `pax info` | `pax.info.v1` | `schemaVersion: "1"` |
| `pax doctor` | `pax.doctor.v1` | `schemaVersion: "1"` |
| `pax deps` | `pax.deps.v1` | `schemaVersion: "1"` |
| `pax scripts` | `pax.scripts.v1` | `schemaVersion: "1"` |
| `pax workspaces` | `pax.workspaces.v1` | `schemaVersion: "1"` |
| `pax lock` | `pax.lock.v1` | `schemaVersion: "1"` |
| `pax graph` | `pax.graph.v1` | `schema_version: "1"` |
| `pax reality` | `pax.reality.v1` | `schema_version: "1"` |
| `pax drift` | `pax.drift.v1` | `schema_version: "1"` |
| `pax observe` | `pax.observation.v1` | — |
| `pax --json test` | `pax.execution-result.v1` | — |
| `pax --dry-run --json ...` | `pax.plan.v1` | — |
| this contract | `pax.contract.v1` | — |

Rules:

- `schema` is the canonical identity. The major version is part of the string.
- The legacy numeric field (`schemaVersion` or `schema_version`) remains `"1"`
  for existing consumers. New consumers should read `schema` and may ignore the
  legacy field.
- A consumer must reject or explicitly handle an unknown `schema`. Do not infer
  semantics from the command name alone.
- An additive field is a `v1` change. Removing a field, changing a type, or
  changing the meaning of an existing value requires a new major schema.
- `pax.observation.v1` and `pax.execution-result.v1` keep their existing shape
  and semantics. New fields are additive only.

## Outcomes

Inspection and observation documents carry a typed top-level `outcome`:

| Outcome | Meaning | Successful? |
| --- | --- | --- |
| `ok` | The observation completed and every subject it reports was established. | yes |
| `partial` | Some facts were established; at least one subject was not. | **no** |
| `unsupported` | PAX has no observer for the requested ecosystem, scope, or subject. | **no** |
| `blocked` | A bound, precondition, or required input prevented the observation. | **no** |
| `failed` | PAX attempted the observation and it did not complete. | **no** |
| `stale` | The observation is older than its subject. Reserved; no observer emits it yet. | **no** |

Only `ok` is success. `partial` is explicitly not success: a consumer that
treats it as success has lost the distinction this contract exists to preserve.

`outcome` is named differently from command-specific status fields on purpose:

- `pax observe` retains its historical `status` (`ok` / `partial` / `error`) and
  adds `outcome`.
- `pax drift` retains its subject-level `status` (`match` / `drift` /
  `ambiguous` / `unsupported`) and adds `outcome`.
- `pax --json test` retains its `pax.execution-result.v1` `status`
  (`passed` / `failed` / `error` / `unsupported` / `ambiguous` / `not_run`).
  It is a result contract, not an observation document, and does not gain
  `outcome`.

### How inspection outcomes are assigned

| Command | `unsupported` when | `partial` when |
| --- | --- | --- |
| `info`, `doctor` | nothing supported was detected | an error-level diagnostic remains |
| `deps` | only container components were detected | an error-level diagnostic remains |
| `scripts` | no JavaScript component was detected | more than one component is present |
| `workspaces` | neither JavaScript nor Rust was detected | a Python component is present |
| `lock` | no JavaScript, Rust, or Python component was detected | multiple Python lockfiles leave no single selected lockfile, or an error-level diagnostic remains |
| `graph`, `reality` | no component was detected | reality reports an `unknown` layer |
| `drift` | no component was detected | a live/runtime issue is `unknown` |

An error-level *diagnostic* does not mean the whole command failed; it means the
command established part of its subject. That is `partial`, not `failed`.

### Observation errors

`pax observe` errors keep their existing document, `status: "error"`, stderr, and
exit code `2`. They add `outcome` using a fixed mapping:

| Observation error code | `outcome` |
| --- | --- |
| `unsupported_project`, `unsupported_scope` | `unsupported` |
| `limit_exceeded` | `blocked` |
| `invalid_scope`, `invalid_limit`, `scope_not_found`, `artifact_unreadable` | `failed` |

A successful `pax observe` with only unsupported diagnostics and no established
facts reports `outcome: "unsupported"` (while retaining `status: "partial"`).
It is never reported as `ok`.

## Fact states

A fact is not a boolean. These states are distinct and must not be collapsed:

| State | Meaning | Example |
| --- | --- | --- |
| `present` | PAX observed the subject as existing. | `node_modules/` is a directory |
| `absent` | PAX looked and the subject does not exist. This is a successful observation. | `node_modules/` is not present |
| `unknown` | PAX did not establish the subject. Nothing was observed either way. | a Python environment directory was not found by convention; the environment may exist elsewhere |
| `unsupported` | PAX has no observer for the subject. | `pax observe` source-structure parsing for Python |
| `blocked` | A bound or precondition prevented the observation. | a `--max-files` limit was exceeded |
| `failed` | PAX attempted the observation and it did not complete. | an invalid scope request |
| `stale` | The fact is older than its subject. Reserved; no observer emits it yet. | — |

`absent` is never `unknown`, `unknown` is never `unsupported`, and `unsupported`
is never success. Commands that cannot observe a subject say so rather than
returning an empty successful result.

`pax.observation.v1` predates this vocabulary. Its per-diagnostic `state` keeps
the existing values for compatibility and maps onto the canonical states:

| Observation diagnostic `state` | Canonical `FactState` |
| --- | --- |
| `unsupported` | `unsupported` |
| `unresolved` | `unknown` |
| `unreadable`, `unparseable` | `failed` |

The observation document's top-level `outcome` uses the canonical vocabulary.
The per-diagnostic `state` is retained unchanged, so a consumer reading an old
document still sees the old words.

## Evidence and provenance

Every fact is traceable to a source. The canonical evidence shape is:

```json
{
  "source": "Cargo.toml",
  "method": "fs.stat",
  "strength": "observed",
  "locator": "line 1"
}
```

| Field | Meaning |
| --- | --- |
| `source` | project-relative file path, or the name of a tool/runtime source |
| `method` | how the evidence was obtained (`fs.stat`, `cargo.metadata`, `syn.parse_file`, …) |
| `strength` | `declared`, `observed`, `resolved`, or `verified` |
| `locator` | optional line number, field, or command selector |

Strength is never upgraded without evidence:

- `declared` — a project artifact states it; nothing checked it.
- `observed` — PAX read the artifact on this machine at observation time.
- `resolved` — a resolver (Cargo) established it.
- `verified` — execution established it. Observation never emits this.

Existing shapes are unchanged:

- `pax observe` facts keep `provenance: { strength, method, source }` plus an
  optional `location`.
- `graph` keeps per-edge `evidence` entries listing the files that established
  the edge.
- `drift` keeps per-issue evidence with `source` and layer `kind`.
- Every inspection `nativeDependencies[]` entry names its declaring
  `source` (a project-relative manifest or requirements file). Graph edges use
  the same file as evidence.

## Entity identities

Identities are stable and machine-independent:

- The project root is `.`.
- Paths are project-relative and use `/`.
- Absolute paths, Windows drive prefixes, UNC paths, and `..` traversal are
  never canonical identities.
- Dependency identities are ecosystem names (`react`, `serde`); workspace
  identities use package names, not machine paths.
- A path outside the project root is represented by an opaque `external/<name>`
  identity rather than an absolute path.

`display_path` and `contract::project_relative_path` enforce this. Graph nodes,
edges, and evidence are covered by regression tests.

## Corrections in v0.5.0

These are intentional corrections, not silent behavior changes:

1. **Cargo workspace root identity.** A workspace root at the project root is
   `.`, not an empty string or an absolute path. Graph evidence and edges use the
   same identity.
2. **Single-package Cargo is not a workspace.** `pax workspaces` reports
   `enabled: false` with no packages, and `pax graph` emits no
   `workspace-member` node or edge for a non-workspace package.
3. **Dry-run plans declare their schema.** `pax --dry-run --json` now includes
   `schema: "pax.plan.v1"`. It still has no `outcome` or `status`; a plan is not
   an observation. The previous absence of `schema` was an undocumented gap.
4. **Drift distinguishes unsupported projects.** With nothing to observe,
   `pax drift` reports `status: "unsupported"` and `outcome: "unsupported"`
   instead of `match`.
5. **The command word is parsed, not searched.** `pax run drift` and
   `pax --dir drift info` no longer select drift exit-code semantics just because
   an argument is the word `drift`.
6. **Python declarations are interpreted.** `pyproject.toml`, standalone and
   nested requirements files, and `Pipfile` declarations name their source;
   malformed manifests produce error diagnostics and `partial`, not invented
   facts or an empty success. `pax deps` no longer reports `partial` merely
   because a Python component exists.
7. **Python lockfiles are inspectable.** `pax lock` includes Python component
   lockfiles and selects the first when no JavaScript or Cargo lockfile exists.
8. **Nested JavaScript manifests are components.** Every `package.json` is
   discovered; nested declarations enter `nativeDependencies` with a
   project-relative `source`, and graph edges are attributed to the declaring
   component. Hoisted `node_modules/` satisfies nested install checks.
9. **Compose services are structural.** Only direct children of `services:`
   are services; nested keys such as `ports` and `environment` are not.
10. **Pipenv is discovery-only.** `Pipfile` declarations are reported with
    `tool: "pipenv"`, but pipenv operations fail with an explicit unsupported
    error rather than guessing a command.

## Compatibility policy

- Existing commands keep their exit codes and existing fields.
- New fields are additive within a major schema version. Consumers should ignore
  fields they do not know.
- Existing `pax.observation.v1` `status` values and error documents are
  unchanged; `outcome` is additive.
- `pax.execution-result.v1` is unchanged.
- `schema` supersedes the legacy numeric version for identification. Both are
  emitted where the legacy field already existed.
- A consumer seeing `outcome: "partial"` or `"unsupported"` must not treat the
  document as a successful observation, even if the process exit code is `0`.
- A consumer seeing `outcome: "failed"` must not treat the document as an empty
  result.

## Fixtures and tests

Fixtures in `tests/fixtures/contract/` are representative documents:

| Fixture | Pins |
| --- | --- |
| `contract.json` | the outcome, state, strength, and identity vocabularies |
| `info.json` | an inspection document with legacy version and evidence |
| `graph.json` | project-relative graph identities and edge evidence |
| `reality.json` | layered presence observations with `absent` |
| `drift.json` | drift subject status plus a successful `outcome` |
| `observation-partial.json` | `pax.observation.v1` with unsupported diagnostics |
| `observation-unsupported.json` | an unsupported error is not success |
| `plan.json` | a plan schema without an outcome |
| `execution-result.json` | the retained execution-result taxonomy |

`tests/contract.rs` additionally runs PAX and checks live output: schema naming,
outcome vocabulary, distinct `absent`/`unknown`/`unsupported`/`failed` states,
evidence on every edge, no absolute graph identities, single-package Cargo, plan
schema, execution-result compatibility, and `pax run drift`.

## Non-goals

This contract does not:

- add new observers, resolution, or execution semantics;
- make `reality` a verification system;
- assign meaning to facts or decide what a consumer should do;
- replace `docs/PAX_BOUNDARY.md`, `docs/PAX_OBSERVATION.md`, or the audit. Those
  documents describe behavior and known gaps; this one describes the wire
  contract.
