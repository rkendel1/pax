# Changelog

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
