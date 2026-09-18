# PAX

PAX is the universal, read-only project-tooling boundary.

It inspects JavaScript, Python, Rust, and Docker projects through one fast Rust
CLI without trying to replace the underlying native tool.

## Commands

```bash
pax info
pax doctor
pax deps
pax scripts
pax workspaces
pax lock
pax --json info
pax --json doctor
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