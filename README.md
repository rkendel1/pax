# PAX

PAX is the universal package-tooling boundary for JavaScript.

It inspects npm, pnpm, Bun, and Yarn projects through one fast Rust CLI without trying to replace the underlying package manager.

## Commands

```bash
pax info
pax doctor
pax --json info
pax --json doctor
```

`pax info` reports detected package-manager reality for the current repository, including lockfile, workspace, and manager-selection evidence.

`pax doctor` reports the same detection data plus core diagnostics for:

- `package.json`
- lockfile presence
- package-manager and lockfile consistency
- workspace configuration

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