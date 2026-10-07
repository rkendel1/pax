PAX v0.3.0 · code audit · branch pax-audit
PAX Audit Findings
PAX is a file-reading project inspector with a thin layer that delegates to native tools. Its “reality” output describes which files and directories exist. It does not verify anything about the project’s real state, and two behaviors claim more than they do.

What PAX does today
Observes
Manifests, lockfiles, workspace and Docker files exist
A few directories exist (node_modules, target, .venv)
Contents of package.json
Cargo’s own metadata, via cargo metadata --no-deps --offline
Derives
Ecosystem and package manager, with the reason
Declared dependency graph
Drift issues, execution plans
Cargo test results (pax.execution-result.v1)
Does not do
Resolve or install dependencies
Detect host tools or runtime versions
Read lockfile contents
Record timestamps or provenance
Run workloads on its own authority
Findings
Each finding was reproduced by running the binary. tests/boundary.rs pins every one as a tripwire.

P0
--live observes nothing
The runtime layer reports status: "unknown" with evidence docker compose ps, but no command runs. On Docker projects, drift --live then returns top-level ambiguous and exit 2, although nothing is ambiguous. Text mode exits 1.

P0
Python parser reports metadata as dependencies
A pyproject.toml with name, version and requires-python yields dependencies named exactly that. The real dependency is never extracted. A requirements.txt on its own yields none, because its reader is nested inside the pyproject loop.

P1
“Reality” layers are presence checks
declared: present means a manifest file exists. resolved: present means a lockfile exists. installed: present means a directory exists: an empty node_modules/ counts, and Rust’s target/ is build output, not installed dependencies. doctor’s runtime is always null.

P1
Drift uses substring matching
A declared dependency is “missing from the lockfile” when its name is not found as text. Short names match almost any lockfile. A JavaScript component without node_modules/ reports drift even when it declares no dependencies.

P1
Malformed manifests are mostly silent
Only an unparsable package.json is an explicit error. A broken pyproject.toml still reports a Python project with no diagnostic. A broken Cargo.toml falls back to a line scanner (the source field says so) while diagnostics still say “found”. Python-only or empty directories report “package.json is required”.

P1
Declared runtime requirements are ignored
engines.node, requires-python, rust-version and .nvmrc never appear in any output. For “project needs Node 24, host has Node 22”, PAX observes neither side.

P1
No consistent schema or compatibility policy
info-family documents use schemaVersion; graph, reality and drift use schema_version; dry-run plans have no version. Values can change under the same "1". Only pax.execution-result.v1 has a documented consumer contract.

P2
Other gaps
Compose keys such as ports are reported as services. project.name can be the directory name, and project.root is an absolute machine path. No output is timestamped. run, exec, install, add, remove and deploy spawn native tools, which is execution-like behavior inside PAX.

Holds
What the tests confirm
Observation commands run no package manager, runtime or Docker, and write no files, including with --live. Output is deterministic for fixed files. Tool selection fails closed on ambiguity. Cargo test results distinguish passed, failed, not_run, error, unsupported and ambiguous.

Where PAX stops and Compute begins
Compute reads only pax --json info, deps and scripts. Chip reads only the --json test result. Foundry passes inspection output, including reality and drift, through to users.

Rule	Status
Anything PAX declares that Compute cannot normalize must not silently disappear	Guaranteed only for non-JS/Python ecosystems and for nativeDependencies. Not for workspaces, diagnostics or lockfile conflicts.
JS workspaces are unsupported (Compute docs)	Not enforced: a workspace runs silently as one project.
“PAX defines project and environment requirements” (Compute docs)	Overstated: PAX reports declared dependencies and scripts, not runtime, platform or tool requirements.
Python refusal in Compute	Protective today partly because PAX’s Python output is wrong. Fixing PAX alone changes behavior.
Recommended next steps
Do now
Fix or remove the --live and runtime claims
Fix or fence the Python parser
Write a compatibility policy for schemaVersion: "1"
Compute: refuse JS workspaces explicitly
Do later
One schema convention and per-field provenance
Report declared runtime requirements
Explicit unparseable and unknown states
Decide whether deploy and package mutation stay in PAX
Don’t do
Dependency solving or lockfile satisfiability
Installing runtimes or managing environments
Sandboxing, persistent state, signing
Goal evaluation, retries, LLM interpretation
Full evidence, field tables and the capability matrix are in the repository: docs/PAX_AUDIT.md, docs/PAX_BOUNDARY.md, docs/PAX_CAPABILITIES.md.