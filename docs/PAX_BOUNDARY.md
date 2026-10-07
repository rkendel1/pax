# PAX boundary

Status: derived from the implementation at v0.3.0 (`src/lib.rs`,
`src/execution_result.rs`, `tests/`). Where this document and the code
disagree, the code and tests win. Evidence and gaps are in
[PAX_AUDIT.md](PAX_AUDIT.md); the capability matrix is in
[PAX_CAPABILITIES.md](PAX_CAPABILITIES.md).

## What PAX is

PAX is a project inspection tool with a thin delegation layer. It reads
project-local files (manifests, lockfiles, workspace files, Dockerfiles, Compose
files) and, for Cargo projects, the output of `cargo metadata --no-deps
--offline`, and reports what it found as structured JSON or text: the detected
ecosystems and package managers, the declared dependencies and scripts, the
presence of lockfiles and install directories, and the files each statement
came from. Separately, it can run the project's native tool (`cargo`, `npm`,
`uv`, …) on request and, for `test`, report what that tool's output lets it
establish. It does not resolve, install, provision, deploy, or run application
workloads on its own authority, and it does not know whether a project works.

## Vocabulary

| Term | Meaning in PAX | Example |
| --- | --- | --- |
| **Observation** | Something PAX directly read at invocation time. | `package.json` exists; `node_modules/` is a directory; `cargo metadata` listed a package. |
| **Derivation** | Deterministic computation from observations. | ecosystem = `rust`; selected manager = `pnpm` by `packageManager` field; graph edges. |
| **Claim** | A statement in PAX output. It is only as strong as the observation or derivation behind it, and should carry the files it came from. | `installed: present` |
| **Declared** | Stated by a project file, not checked against anything. | `dependencies` in `package.json`. |
| **Observed** | Seen on this machine now. | a directory exists. |
| **Reality** | Reserved. PAX output may be called *reality* only if it states what was observed, where, when, how it was derived, declared vs. observed, and what is uncertain. **No PAX output meets this bar today**: nothing is timestamped, uncertainty is expressed only as `unknown` in a few places, and several fields are presence checks (see the audit). The command and field names `reality`, `resolved`, and `installed` are retained for compatibility; read them as "layered presence observations". |

## Diagram

```text
                 PROJECT
                    │
                    ▼
                  PAX
          ┌───────────────────┐
          │ inspect           │
          │ detect            │
          │ parse             │
          │ normalize         │
          │ derive            │
          │ report evidence   │
          └─────────┬─────────┘
                    │
          project observations
          / requirements
                    │
                    ▼
                 COMPUTE
          ┌───────────────────┐
          │ resolve           │
          │ plan              │
          │ place             │
          │ execute           │
          │ observe execution │
          │ receipt           │
          └───────────────────┘
```

PAX observes the project. Compute executes the project. Neither should absorb
the other's responsibility.

Two discrepancies with this picture exist today and are documented, not fixed:

1. PAX has a delegation surface (`build`, `test`, `lint`, `typecheck`, `run`,
   `x`, `exec`, `install`, `add`, `remove`, `deploy`) that spawns native tools.
   That is execution-like behavior inside PAX. It is a convenience wrapper over
   the native tool, not a sandbox, scheduler, or evidence producer. The
   boundary rule below applies to it.
2. PAX reports "requirements" only as *declared dependencies and scripts*. It
   reports no runtime version, platform, or tool requirement, and it does not
   state requirements in a normalized form. Compute's documentation describing
   PAX as defining "project and environment requirements" is stronger than the
   implementation (see audit).

## Boundary rules

1. **Observation commands are read-only.** `info`, `doctor`, `deps`, `scripts`,
   `workspaces`, `lock`, `graph`, `reality`, `drift` (with or without `--live`)
   run no package manager, runtime, or Docker, and write no files. The only
   process they start is `cargo metadata --no-deps --offline` for projects with a
   `Cargo.toml`. Enforced by `tests/boundary.rs`.
2. **Delegation commands run the native tool as the user would**, preserve its
   exit status, and may mutate the project exactly as that tool does. PAX adds
   tool selection (fail-closed on ambiguity) and, for `--json test`, an
   interpreted result (`pax.execution-result.v1`).
3. **PAX does not decide success of anything beyond what it can establish
   from native evidence.** The execution result's `status` is about the
   delegated operation, not about a consumer's goal.
4. **PAX does not hold state.** No cache, database, daemon, or sidecar file.
5. **A consumer must not treat a PAX presence check as a satisfied
   requirement.** "Declared", "present", and "ran successfully" are different
   facts.

## PAX does not

| Area | Statement | Basis |
| --- | --- | --- |
| Execution of workloads | Observation commands never execute anything the project defines. Delegation commands execute the *native tool* the user asked for; PAX does not place, isolate, schedule, meter, or produce execution receipts. | `tests/boundary.rs::observation_commands_execute_no_tool_and_write_nothing` |
| Package management | PAX replaces none of Cargo, npm, pnpm, yarn, bun, uv, pip, Poetry, PDM. `install`/`add`/`remove` delegate. | `build_install_command`, `build_package_mutation_command` |
| Dependency resolution | PAX reads declared specifiers, lockfile *presence*, and Cargo's own metadata. It does not solve versions and does not parse lockfiles (drift does a substring search for names). | `build_drift`, `detect_cargo` |
| Environment management | PAX creates no virtualenv, `node_modules`, toolchain, or container. It checks whether a few conventional directories exist. | `build_reality` |
| Deployment | `deploy` delegates to `fly`/`vercel`/`netlify` selected from config-file evidence. PAX does not deploy on its own and says nothing about deployability. | `select_deploy_provider` |
| Runtime management | PAX does not install, select, or probe Node, Python, or Rust versions. `doctor`'s `runtime` field is always `null`. | `detect_node_runtime` |
| Security / provenance | PAX output is not provenance. A tool selected by name is not verified (no path, hash, or signature is recorded). | no such code exists |
| Application semantics | PAX does not read source code. It reads manifests. | `detect_*` |
| Business semantics | Out of scope. | — |
| Agent reasoning | PAX is not Chip; it contains no model, planner, retry, or repair logic. | — |

## Where PAX stops and Compute begins

Compute consumes `pax --json info|deps|scripts` through one adapter
(`compute/crates/compute-project/src/pax.rs`) and nothing else from PAX.
Everything after that point (planning, placement, materialization, execution,
receipts) is Compute's. PAX has no knowledge of Compute: no Compute types,
fields, or protocols appear in this repository.

| Fact | Owner |
| --- | --- |
| A project declares dependency `X ^1` | PAX (declared observation) |
| Lockfile `Y` exists next to the manifest | PAX (observation) |
| Package manager selected by evidence | PAX (derivation) |
| The host has Node 22 | nobody in PAX; Compute observes target capabilities |
| The project needs Node 24 | declared by the project; **not read by PAX today** |
| The requirement is satisfied | Compute, from its own evidence |
| The workload ran, its output, its receipt | Compute |

Declared requirement, observed host capability, resolved requirement, and
verified execution are four different things. PAX can currently produce only
the first (partially) and a few filesystem presence facts. It never compares
them and does not claim to.

## What other systems own

| Capability | Owner |
| --- | --- |
| Execution, placement, machines, isolation, receipts | Compute |
| Dependency resolution and installation | package managers |
| Compilation and artifacts | build systems (Cargo, tsc, …) |
| Revision identity and repository provenance | Git / source control |
| Application and service capability boundaries | AppPort |
| Durable authoritative state | FeltDB |
| Reasoning, planning, agent behavior | Chip |
| Goals, priorities, approvals, human attention | Attn |

## Deliberately outside PAX

Even if useful, PAX should not acquire: a dependency solver; a lockfile
generator or full lockfile parser that decides satisfiability; runtime or
toolchain installers; environment creation; sandboxing; a persistent
observation store; fleet or machine inventory; cryptographic provenance or SBOM
signing; vulnerability analysis; goal evaluation; retries or repair; LLM
interpretation. PAX may *report* the inputs these systems need.
