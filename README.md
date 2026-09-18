# PAX

PAX is the universal, read-only project-tooling boundary.

It inspects JavaScript, Python, Rust, and Docker projects through one fast Rust
CLI without trying to replace the underlying native tool.

“Read-only” describes PAX's observation model. Delegated commands such as
`pax run`, `pax install`, `pax add`, `pax remove`, and `pax deploy` execute the
selected native tool and may mutate project state exactly as that tool normally
would.

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

`pax info` reports detected package-manager reality for the current repository, including lockfile, workspace, and manager-selection evidence.

`pax doctor` reports the same detection data plus core diagnostics for:

- `package.json`
- lockfile presence
- package-manager and lockfile consistency
- workspace configuration

PAX also detects Python (`uv`, `pip`, Poetry, PDM), Rust (`cargo`), and Docker
or Compose projects. `pax info --json` reports all detected components in a
repository, including manifests, lockfiles, tool evidence, native dependency
semantics, and static container services. Inspection never invokes Node,
Python, Cargo, package managers, or the Docker daemon.

`pax deps`, `pax scripts`, `pax workspaces`, and `pax lock` expose normalized
package metadata, script definitions, workspace configuration, and lockfile
state. All commands support `--json`; PAX only reads repository files and
never invokes a package manager.

`pax graph` reports project components and native dependency relationships with
ecosystem-specific types and evidence. `pax reality` separates declared,
resolved, installed, and runtime observations; runtime inspection is opt-in
with `--live`. `pax drift` reports contradictions between those layers and
never repairs them. Static observation does not require package managers,
network access, or a Docker daemon.

`pax drift` uses exit code 0 for no drift, 1 for detected drift, and 2 for
ambiguous observations. Invalid input uses exit code 2.

`pax run <target> [args...]` delegates to the detected native tool without
interpreting the target: JavaScript uses the selected package manager, Python
uses `uv`, Poetry, PDM, or Python, Rust uses Cargo, and Compose uses Docker
Compose. Standard input/output/error, environment, working directory, and the
delegated process exit status are preserved.

`pax x <package> [args...]` delegates package execution to the ecosystem's
native runner, such as `npx`, `pnpm dlx`, `bunx`, `yarn dlx`, `uvx`, or `pipx`.
`pax x install [args...]` delegates dependency installation to the detected
native package manager, including commands such as `pip install -r
requirements.txt`.

`pax install [package...]` is the universal installation entry point. It
delegates both project installs and package additions to the authoritative
ecosystem tool without reimplementing package-manager behavior.

The command vocabulary is intentionally narrow:

- `pax run` — project task runner
- `pax x` — ephemeral package/tool runner
- `pax install` — dependency installation
- `pax add` / `pax remove` — dependency mutations
- `pax exec` — exact native command escape hatch

This is the complete v0.1 API surface; no additional commands are implied.

PAX resolves the ecosystem tool and delegates to it; it does not replace npm,
pnpm, Yarn, Bun, uv, pip, Poetry, PDM, Cargo, or Docker. Use `--tool` for an
explicit override and `--dry-run` (optionally with `--json`) to inspect the
execution plan without running it.

`pax deploy` detects Fly.io (`fly.toml`), Vercel (`vercel.json` or
`.vercel/project.json`), or Netlify (`netlify.toml`) evidence and delegates to
the provider CLI. Use `--tool fly`, `--tool vercel`, or `--tool netlify` to
disambiguate or explicitly select a provider. `--dry-run` reports the selected
provider, evidence, and canonical command without executing it.

## Architecture audit contract

PAX owns detection, planning, observation, and evidence. Native tools remain
authoritative for execution and ecosystem semantics; PAX does not resolve
dependencies, implement a registry, or replace a package manager, build system,
shell, container runtime, or deployment provider.

| Command | Boundary | Mutates | External process | Evidence |
| --- | --- | --- | --- | --- |
| `info`, `doctor`, `deps`, `scripts`, `workspaces`, `lock` | native inspection | no | no | project files |
| `graph`, `reality`, `drift` | native observation | no | only `--live` | manifests, locks, installed/runtime observations |
| `run`, `x`, `install`, `add`, `remove` | delegated/composite | install or mutation commands may | yes | detected or overridden tool |
| `exec` | exact delegation | depends on supplied command | yes | user-supplied command |

Tool selection is deterministic and fail-closed: an explicit `--tool` override
is selected first, then a recognized `packageManager` field, then lockfile
evidence. Contradictory lockfiles without an override are reported as
`ambiguous`; PAX never silently resolves the conflict. Use `--dir` to select a
specific project root. Use `--` when arguments to a delegated command must not
be interpreted as PAX flags.

JSON output is a versioned machine-readable contract. Observation statuses are
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

The v0.1 release target matrix is intentionally explicit:

| Platform | Rust target |
| --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

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