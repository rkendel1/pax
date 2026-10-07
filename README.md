# PAX

PAX inspects project artifacts and selected tooling information and produces
structured observations. It can also delegate to the project's native tool.

It reads JavaScript, Python, Rust, and Docker projects through one Rust CLI
without trying to replace the underlying native tool. It does not resolve
dependencies, manage environments or runtimes, or run workloads on its own.
See [docs/PAX_BOUNDARY.md](docs/PAX_BOUNDARY.md) for what PAX claims and does
not claim, [docs/PAX_CAPABILITIES.md](docs/PAX_CAPABILITIES.md) for the
capability matrix, and [docs/PAX_AUDIT.md](docs/PAX_AUDIT.md) for the evidence
and known gaps.

The inspection and observation commands (`info` through `drift`) are
read-only. Delegated commands such as
`pax build`, `pax test`, `pax lint`, `pax typecheck`, `pax run`, `pax install`,
`pax add`, `pax remove`, and `pax deploy` execute the selected native tool and
may mutate project state exactly as that tool normally would.

## Commands

```bash
pax info
pax doctor
pax deps
pax scripts
pax workspaces
pax lock
pax graph
pax reality
pax drift
pax reality --live
pax drift --live
pax build
pax test
pax lint
pax typecheck
pax run dev
pax x prettier
pax install
pax install react
pax add react
pax remove react
pax exec npx prisma generate
pax deploy --dry-run
pax --json info
pax --json doctor
pax --json graph
pax --json reality
pax --json drift
pax --dir path/to/project info
pax --help
pax --version
```

`pax info` reports the detected ecosystems, the selected package manager and why it was selected, lockfiles, workspaces, and declared dependencies for the current repository.

`pax doctor` reports the same detection data plus core diagnostics for:

- `package.json`
- lockfile presence
- package-manager and lockfile consistency
- workspace configuration

PAX also detects Python (`uv`, `pip`, Poetry, PDM), Rust (`cargo`), and Docker
or Compose projects. `pax info --json` reports all detected components in a
repository, including manifests, lockfiles, tool evidence, native dependency
semantics, and static container services. Cargo projects use read-only,
offline `cargo metadata --no-deps` for authoritative workspace semantics.
Inspection never invokes Node, Python, JavaScript/Python package managers, or
the Docker daemon.

`pax deps`, `pax scripts`, `pax workspaces`, and `pax lock` expose normalized
package metadata, script definitions, workspace configuration, and lockfile
state. All commands support `--json`; PAX does not install, resolve, or mutate
dependencies while inspecting a project.

`pax graph` reports the declared dependency and workspace relationships with
ecosystem-specific types and evidence. `pax reality` reports layered *presence*
observations: which manifests exist (`declared`), which lockfiles exist
(`resolved`), whether conventional install directories exist (`installed`), and
`runtime`. It does not read lockfile contents or check installed packages, and
`--live` does not currently observe anything: the `runtime` layer reports
`unknown`. `pax drift` reports contradictions between those layers and never
repairs them. Static observation does not require network access or a Docker
daemon.

Known limitations (Python and Compose parsing, unparseable manifests, no
runtime or tool detection) are listed in [docs/PAX_AUDIT.md](docs/PAX_AUDIT.md).

`pax drift` uses exit code 0 for no drift, 1 for detected drift, and 2 for
ambiguous observations. Invalid input uses exit code 2.

`pax run <target> [args...]` delegates to the detected native tool without
interpreting the target: JavaScript uses the selected package manager, Python
uses `uv`, Poetry, PDM, or Python, Rust uses Cargo, and Compose uses Docker
Compose. Standard input/output/error, environment, working directory, and the
delegated process exit status are preserved.

`pax build`, `pax test`, `pax lint`, and `pax typecheck` are first-class project
operations. PAX resolves each operation once, reports why it selected the
native tool, and delegates execution. In JavaScript projects these commands use
the corresponding package script; in Rust projects they map to Cargo's native
`build`, `test`, `clippy`, and `check` operations. Use `pax run <operation>` for
arbitrary project-defined operations.

`pax x <package> [args...]` delegates package execution to the ecosystem's
native runner, such as `npx`, `pnpm dlx`, `bunx`, `yarn dlx`, `uvx`, or `pipx`.
`pax x install [args...]` delegates dependency installation to the detected
native package manager, including commands such as `pip install -r
requirements.txt`.

`pax install [package...]` is the universal installation entry point. It
delegates once to the authoritative tool for the selected project root. The
native package manager owns dependency resolution and fetching; PAX does not
walk the dependency graph or issue one fetch per component.

The command vocabulary is grouped by intent:

- `pax build` / `test` / `lint` / `typecheck` — universal project operations
- `pax run` — project task runner
- `pax x` — ephemeral package/tool runner
- `pax install` — dependency installation
- `pax add` / `pax remove` — dependency mutations
- `pax exec` — exact native command escape hatch

PAX resolves the ecosystem tool and delegates to it; it does not replace npm,
pnpm, Yarn, Bun, uv, pip, Poetry, PDM, Cargo, or Docker. Use `--tool` for an
explicit override and `--dry-run` (optionally with `--json`) to inspect the
execution plan without running it.

`pax deploy` detects Fly.io (`fly.toml`), Vercel (`vercel.json` or
`.vercel/project.json`), or Netlify (`netlify.toml`) evidence and delegates to
the provider CLI. Use `--tool fly`, `--tool vercel`, or `--tool netlify` to
disambiguate or explicitly select a provider. `--dry-run` reports the selected
provider, evidence, and canonical command without executing it.

## Execution result contract

`pax --dry-run --json test` produces a structured **plan**: what PAX will
execute. `pax --json test` produces a structured **execution result**: what
happened when PAX executed it. They are different contracts; the plan has no
`schema` or `status` field and its meaning is unchanged.

The execution result describes what PAX established about the delegated
operation. It is not a general-purpose goal evaluator: whether a consumer's
larger goal is satisfied remains the consumer's decision. Only `test` has an
execution-result contract today. Without `--json`, `pax test` behaves exactly
as before.

stdout carries exactly one JSON document. Native stdout, native diagnostics, and
PAX diagnostics go to stderr. Example:

```json
{
  "schema": "pax.execution-result.v1",
  "operation": "test",
  "status": "failed",
  "reason": "tests-failed",
  "tool": "cargo",
  "exit_code": 101,
  "tests": { "passed": 1, "failed": 1, "ignored": 0, "measured": 0 }
}
```

| Field | Presence | Meaning |
| --- | --- | --- |
| `schema` | always | `pax.execution-result.v1`. Consumers must reject or explicitly handle any other value; an incompatible semantic change requires a new identifier. |
| `operation` | always | `test` |
| `status` | always | `passed`, `failed`, `error`, `unsupported`, `ambiguous`, `not_run` |
| `reason` | always | stable machine code refining `status` (below) |
| `tool` | always | selected native tool, or `null` if none was selected |
| `exit_code` | always | the native process exit status, unmodified (`101` stays `101`); `null` if no native process ran to a normal exit |
| `tests` | optional | `passed`, `failed`, `ignored`, `measured` summed over every libtest summary; present only when every summary line parsed |

The semantic fields contain no timestamps, process ids, or paths. Process exit
status of `pax` itself mirrors the native status when a process ran, and PAX's
existing fail-closed codes otherwise (`2` ambiguous/unsupported selection, `1`
error).

| `status` | `reason` | Established when |
| --- | --- | --- |
| `passed` | `tests-passed` | Cargo exited 0, every libtest summary was observed, and at least one test passed. |
| `failed` | `compilation-failed` | Cargo's `build-finished` message reports `success: false`. |
| `failed` | `tests-failed` | A libtest summary reports failed tests. |
| `failed` | `native-exit-nonzero` / `terminated-by-signal` | Cargo ran and exited non-zero (or was killed) without more specific evidence. |
| `not_run` | `no-tests-executed` | Cargo exited 0 and libtest reports zero passed and zero measured tests (zero tests, or only ignored tests). |
| `unsupported` | `no-libtest-evidence` | Cargo exited 0 but PAX cannot establish that tests executed (`--no-run`, `-- --list`, `harness = false` targets, unparseable summaries). Exit 0 alone is not treated as a pass. |
| `unsupported` | `interpretation-unsupported` | The selected tool (npm/pnpm/yarn/bun scripts, Python, Docker) is run and its `exit_code` preserved, but PAX has no authoritative test semantics for it. Do not treat `exit_code` as semantic. |
| `unsupported` | `operation-unsupported` | PAX's existing selection refuses the operation (for example no `test` script). Nothing is executed. |
| `ambiguous` | `ambiguous-selection` | PAX's existing selection is ambiguous (multiple lockfiles). Nothing is executed and no fallback tool is chosen. |
| `error` | `launch-failed` | The native tool could not be launched. Never reported as `failed`. |
| `error` | `invalid-project-directory` / `detection-failed` | PAX could not inspect the project. |

For Cargo, PAX runs `cargo test --message-format=json-render-diagnostics`
(skipped if you pass your own `--message-format`) and reads cargo's stable
`build-finished` and artifact messages. libtest's JSON format is nightly-only,
so counts come from its stable `test result:` summary lines; they are optional
and omitted rather than guessed. Doctests, multiple test targets, and workspaces
are covered because every summary is summed.

Zero tests: a project in which no test executes reports `not_run` with
`exit_code: 0`, never `passed`.

## Architecture audit contract

PAX owns detection, planning, observation, and evidence. Native tools remain
authoritative for execution and ecosystem semantics; PAX does not resolve
dependencies, implement a registry, or replace a package manager, build system,
shell, container runtime, or deployment provider.

| Command | Boundary | Mutates | External process | Evidence |
| --- | --- | --- | --- | --- |
| `info`, `doctor`, `deps`, `scripts`, `workspaces`, `lock` | native inspection | no | Cargo metadata for Rust | project files and Cargo metadata |
| `graph`, `reality`, `drift` | native observation | no | Cargo metadata; runtime only with `--live` | manifests, locks, installed/runtime observations |
| `build`, `test`, `lint`, `typecheck` | resolved project operation | depends on native tool | yes | detected or overridden tool |
| `run`, `x`, `install`, `add`, `remove` | native delegation | install or mutation commands may | yes | detected or overridden tool |
| `exec` | exact delegation | depends on supplied command | yes | user-supplied command |

Tool selection is deterministic and fail-closed: an explicit `--tool` override
is selected first, then a recognized `packageManager` field, then lockfile
evidence. Contradictory lockfiles without an override are reported as
`ambiguous`; PAX never silently resolves the conflict. Use `--dir` to select a
specific project root. Use `--` when arguments to a delegated command must not
be interpreted as PAX flags.

`--dry-run` is the authoritative execution contract: it uses the same resolved
operation as execution without starting the native process. Add `--json` for
the structural representation, including the operation, project root,
workspace, selected tool, native command, environment additions, working
directory, support status, evidence, and selection reason.

JSON output carries a version (`schemaVersion` or `schema_version`, currently
`"1"`) but no compatibility policy has been written for observation
documents; only `pax.execution-result.v1` has a documented consumer contract. Observation statuses are
`match`, `drift`, `ambiguous`, or `unknown`; unknown evidence is never promoted
to drift. Exit code `0` means success/match, `1` means delegated failure or
drift, and `2` means invalid input or ambiguity.
JSON is intended for automation and agents; human-readable diagnostics are
written separately and are never mixed into JSON output.

## Installation

Release tags publish native archives containing the `pax` executable. Download
the archive for your platform from the
[GitHub Releases](https://github.com/rkendel1/pax/releases) page, extract it,
and put the executable on `PATH`. No runtime other than the operating system is
required:

```bash
pax --version
pax --help
pax info
```

The v0.2 release target matrix is intentionally explicit:

| Platform | Rust target |
| --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

Each release includes a `.tar.gz` archive for macOS and Linux, a `.zip`
archive for Windows, and a `SHA256SUMS` file. Archives contain the native
executable together with `README.md` and `LICENSE`.

These are the supported release artifacts; other platforms are not advertised
until they have reliable release builds and tests. Building from source remains
available for contributors with Rust installed:

```bash
cargo install --path .
```

PAX uses native executable names and path handling supplied by the operating
system. Delegated tools remain external authorities and must be installed
separately when a command needs them.

## Detection model

PAX detects:

- `package.json`
- `package-lock.json`
- `pnpm-lock.yaml`
- `yarn.lock`
- `bun.lock`
- `bun.lockb`
- `pnpm-workspace.yaml`
- the `packageManager` field

Manager selection is deterministic:

1. A recognized `packageManager` field wins.
2. Otherwise PAX falls back to lockfile precedence.
3. PAX reports the evidence it used so agents and humans can see why a manager was selected.

## Development

```bash
cargo fmt
cargo test
```
