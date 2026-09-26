# Design

## Context

See proposal.md - Why. `mf-cli::compile` currently obtains `mf_compiler::plugin_registry()` before generating a project. That registry comes from the fixed `mf-bundle`. The CLI then locates runtime and bundle paths in a source checkout. Adding dependencies only to the generated runner cannot work because the CLI rejects unfamiliar kinds first.

The compiler already accepts an explicit registry for validation and code generation. Nodes implement `Node`, submit `NodeRegistration` through `inventory`, and have factories that run during configuration validation and again during runner initialization. These contracts remain usable.

## Goals / Non-Goals

**Goals:**

- Let the same installed CLI build unrelated projects with different third-party nodes.
- Keep plugin selection, dependency versions, and features consistent between validation and execution.
- Retain a standalone native executable and the existing DAG validation and deterministic execution order.
- Reuse generated projects and Cargo artifacts across builds while validating current inputs on every invocation.
- Make dependency and build failures inspectable without overwriting an existing executable.

**Non-Goals:**

- Dynamic library loading, plugin installation into the CLI, or a plugin marketplace.
- Cross-compilation, automatic Rust installation, hermetic builds, or bit-for-bit reproducible binaries.
- An untrusted-code sandbox, remote build service, or dependency cache manager.
- A separate project manifest, shared dependency inheritance across Flows, changes to the node execution API, or scheduling changes.

## Decisions

### 1. Declare dependencies inside the workflow definition

The Flow JSON is the single user-authored build input. The command remains:

```sh
mf compile flow.json --output ./flow
mf compile flow.json --output ./flow --locked
```

Schema `2026-09-26` adds a required top-level `dependencies` object. It uses the existing date-formatted `version` field rather than a separate manifest version. The new CLI rejects `2026-09-24` with instructions to update the version and declare the packages providing its node kinds. Nodes, configuration, edges, and selected outputs keep their existing semantics.

Example definition, using an illustrative plugin package and port contract:

```json
{
  "version": "2026-09-26",
  "dependencies": {
    "http": {
      "package": "example-http-nodes",
      "version": "1.2",
      "features": ["json"],
      "default-features": false
    }
  },
  "nodes": [
    {
      "id": "fetch",
      "kind": "http.request",
      "config": { "url": "https://example.org/data" }
    }
  ],
  "edges": [],
  "outputs": [{ "name": "response", "node": "fetch", "port": "body" }]
}
```

`package` is required. Each dependency selects exactly one source: a crates.io `version` requirement, a `git` URL with a full commit `rev`, or a local `path`. `features` defaults to empty and `default-features` defaults to true. Unknown fields and conflicting or incomplete sources fail with JSON-field diagnostics such as `dependencies.http.version`. Alternate registries, arbitrary Cargo manifest passthrough, and dependency patches are deferred.

Aliases identify dependencies for diagnostics; they do not prefix node kinds. A crate can register several kinds. All declarations participate in the build even if the definition does not use their nodes. An explicit empty `dependencies` object is valid, but cannot satisfy any node reference. Built-ins follow the same declaration rules as third-party crates; there is no implicit bundle.

Resolve the definition to its canonical path before deriving relative local dependencies and adjacent lock paths, so
invoking through a symlink uses the same inputs. Local dependency paths are relative to that definition's directory,
independently of the invocation directory. Replace the definition's last extension with `.lock`, or append `.lock` if it
has no extension: `flow.json` becomes `flow.lock` and `order.flow.json` becomes `order.flow.lock`. Reject any derived
lock path that aliases the definition itself.

Dependency resolution remains tool-generated state in the separate lock file. Compilation never rewrites the user's JSON. Two definitions in one directory have independent dependencies and locks when their derived lock paths differ; definitions that derive the same lock path share its serialization boundary and must be renamed to obtain independent locks.

Embedding dependencies makes one Flow definition sufficient to identify both its graph and required plugins, without pairing it with another user-authored file. A shared project manifest could reduce repetition across many Flows but is deferred until that use case requires it.

### 2. Generate one runner with validation and execution modes

The CLI parses the definition and dependencies, checks graph structure without a plugin registry, computes deterministic topological order, and emits direct node orchestration. It generates one Cargo binary target with a shared inventory registry and embedded definition. Plugin imports use deterministic safe Rust aliases, with an explicit link anchor for every declared dependency.

The runner accepts `--validate` to resolve kinds, validate ports and configuration, and construct nodes without calling `Node::execute`. Its default mode executes the generated orchestration. The CLI builds once, invokes that exact executable in validation mode, and installs it only after validation succeeds. No separate helper, placeholder target, second compilation, or interpreted graph execution is required.

Build stages:

1. Acquire dependency-file and build-directory locks, then structurally validate the definition and generate runner inputs. Preserve identical files and their modification times.
2. Synchronize the working Cargo lock from the adjacent Flow lock, removing a stale working lock if the authoritative lock is absent; resolve dependencies and verify runtime identity.
3. Build the runner with `--locked`, streaming Cargo diagnostics.
4. Execute the newly built runner with `--validate`. Registry, configuration, and port errors fail the build without executing business logic. Diagnostic stdout is not a machine-readable protocol.
5. Persist the dependency lock when permitted, stage the validated executable beside its destination, and atomically install it. Retain the generated project and target directory.

Plugin validation now happens after code generation and compilation but before artifact installation. This trades earlier diagnostics and a smaller binary for one stable target and fewer build stages. The runner retains validation code and metadata. Configuration factories still run during build validation and normal node construction, so operational I/O belongs in `execute`.

Reuse the existing registration and compiler validation APIs, factoring structural planning from plugin checks. Compiler libraries must not link selected plugins; only the generated runner does. Verify optimized link retention with a fixture exporting multiple registrations without a `kind()` function.

### 3. Keep compiler libraries independent of selected plugins

Remove the production `mf-compiler -> mf-bundle` edge and the implicit `plugin_registry()` convenience API. Library callers continue to provide `NodeRegistry` explicitly. The CLI must not transitively link built-in node crates. Existing registry tests use explicit test fixtures; production selection comes only from the definition's `dependencies` object.

Before running validation, inspect Cargo metadata and require one resolved `mf-runtime` package identity, matching the runner. Reject multiple runtime versions or sources with the relevant dependency paths. Registrations compiled against a different runtime identity would otherwise be invisible to the expected inventory even when types have similar names.

Validation and execution use the same binary, so their package identities and feature sets match. Plugins must not vary registration contracts based on executable identity or external mutable state. Runtime checks for missing registrations and missing output values remain in place.

### 4. Reuse Cargo resolution and persist its lock

The per-definition lock file stores the generated package's Cargo lock content. Generate stable package names and dependency ordering, and synchronize the lock into the reusable package before Cargo resolution. The complete runner graph, including validation dependencies, is resolved before compilation.

Without `--locked`, reuse compatible locked versions and allow Cargo to update entries when required by the current dependency declarations or CLI support packages. Persist the resulting lock atomically after the runner builds, before replacing the executable. With `--locked`, require an existing lock and fail if resolution would change it; never rewrite it. All subsequent Cargo build commands use `--locked` even for an initially unlocked invocation.

Acquire an exclusive OS-backed build lock keyed by the canonical dependency lock path for the lock lifecycle, including
locked invocations. Contending builds fail with a clear retry diagnostic. Release it on exit; stale file presence alone
must not block later builds. On failure before persistence, retain the previous dependency lock. If executable
installation fails after lock persistence, report that the lock was updated and preserve the project; the two separate
paths are not a filesystem transaction.

Protect the definition and lock from use as the executable output, including canonical path aliases. Preserve existing atomic executable replacement behavior.

Cargo remains responsible for version solving and registry/Git fetching. The dependency lock fixes dependency resolution, not local source contents, compiler versions, native libraries, or all build-script inputs. Local path crates remain mutable and are documented as development inputs.

### 5. Reuse build directories across invocations

Default builds use an application-owned directory under the platform's per-user cache root. Select the entry by a stable hash of the canonical definition path, CLI version, and generated-project layout version. The output path is not part of the identity, so changing `--output` still reuses compilation work. Keep dependencies and graph contents out of this directory key; update generated inputs in place and let Cargo determine which compiled artifacts remain fresh.

Users can choose an exact build directory, including a persistent location for CI:

```sh
mf compile flow.json --output ./flow --build-dir /tmp/order-build
mf compile flow.json --output ./flow --build-dir /tmp/order-build --locked
```

An explicit path is relative to the invocation directory. Initialize an absent or empty directory with an ownership marker recording the canonical definition path, CLI version, and layout version. Refuse a nonempty unrecognized directory or an entry for another definition or incompatible layout/version; report the mismatch and request a different or empty directory. Never automatically clear unrelated files. Report the chosen directory and whether it was created or reused.

After the dependency-file lock, acquire an exclusive OS-backed lock for the build directory, using this order
consistently. Lock ownership checks and all mutations occur while holding it; contention fails with a retry diagnostic.
Build directories for different definitions remain independent. Reject layouts that place the definition, its dependency
lock, or final output inside the managed build directory, including canonical aliases. This keeps generated-file
maintenance separate from user inputs and final artifacts.

Persist the generated Cargo package, runner sources, and `target` artifacts after both successful and failed builds. On every invocation, reparse current inputs, restore the working Cargo lock from the authoritative adjacent Flow lock, and invoke Cargo for the runner. Always rerun validation, even when Cargo reuses the binary. Do not treat directory presence or a previous successful executable as evidence that current validation or compilation succeeded.

Write generated files only when bytes change. Generate a complete artifact set in memory before synchronizing owned files, remove obsolete generated configurations, and require successful Cargo compilation and validation in the current invocation before installation. Failed or interrupted attempts can leave useful diagnostics and compiled dependencies but cannot supply validation results for a later attempt.

Graph or configuration edits regenerate runner inputs while retaining dependency artifacts. Dependency, feature, or lock
changes resynchronize the manifest and working lock before compilation. Toolchain, compiler flags, local source, and
tracked build-script inputs follow Cargo's freshness rules; neither CLI caching nor `--locked` bypasses Cargo. Tests
must cover these changes rather than relying on elapsed-time thresholds. CLI/layout version changes select a new default
entry or reject incompatible explicit reuse.

The cache is disposable: if its directory is removed by the user or operating system, the next build recreates it from
the Flow and lock. A partial entry with a valid ownership marker is repaired by resynchronizing generated inputs; an
invalid marker is reported rather than silently trusted. Users may delete inactive build entries to reclaim disk space;
automatic eviction, remote caches, and cross-Flow target sharing are deferred. Cached configurations and diagnostics
remain local build data, protected with user-private directory permissions.

### 6. Ship versioned support crates independently of the checkout

A distributed CLI generates exact-version crates.io dependencies for its matching `mf-runtime` and `mf-compiler` release. Built-in node packages are independently selectable dependencies. Package preparation must include valid publishable dependency metadata and verification that packaged compiler code has no fixed bundle or checkout dependency.

Supporting crates must be available before distributing a CLI release that requires them. Registry name availability and publication credentials are release prerequisites, not assumptions about what is currently published. Do not silently fall back to searching the user's filesystem or downloading a moving branch.

Repository development and integration tests can use an explicit internal package-source override injected by the test/build harness, mapping support crates to local paths or a temporary registry. This is not a public workflow field. A release acceptance test must use packaged crates in an isolated registry and invoke the installed CLI outside the checkout, with no local source override.

Published packages keep the installation model small and align plugin runtime identity with normal Cargo dependencies. Embedding a second source distribution in the CLI would require source extraction and source-identity rewriting for third-party runtime dependencies. It is deferred.

### 7. Preserve failure boundaries

Report failures by stage: definition and dependency parsing, dependency resolution or compatibility, runner compilation, runner validation, lock persistence, or executable installation. Forward Cargo diagnostics and report the retained project location for failures after project creation. Do not treat a cached executable or partially synchronized project as a successful build.

Building third-party Rust code executes build scripts, procedural macros, and configuration factories with the user's permissions. Document this existing native build trust model; this change adds no mandatory approval prompt or sandbox claim. Missing Cargo, Rust, packages, or native libraries produces actionable diagnostics without automatic installation.

## Risks / Trade-offs

- [Repeated builds are expensive] -> Retain generated projects and Cargo targets, avoid rewriting identical files, and verify warm builds reuse fresh dependencies.
- [Stale or interrupted build state is reused] -> Treat current inputs and the Flow lock as authoritative; rerun validation and require fresh per-attempt artifacts before installation.
- [Retained build directories consume disk space] -> Report their locations, support explicit placement, and document deletion of inactive entries.
- [Runtime dependency identities diverge] -> Validate the resolved graph before runner validation and test mismatched versions and sources.
- [Factories have compile-time side effects] -> Document factory expectations and keep their diagnostics visible; isolation from the CLI is not a security boundary.
- [Support crates are unavailable] -> Gate CLI distribution on package availability and exercise the packaged path in release acceptance tests.
- [Users rely on implicit built-ins] -> Provide updated JSON examples and an old-schema migration diagnostic.
- [Local dependencies change despite a lock] -> State the limit of resolution locking; retain native Cargo behavior rather than promising immutable source inputs.

## Migration Plan

1. Remove production bundle coupling while preserving compiler APIs that accept an explicit registry.
2. Add definition and dependency parsing, generated runner validation and execution, dependency lock handling, and reusable build directories.
3. Prepare support and built-in packages for release; validate the packaged path in an isolated registry before publishing a compatible CLI.
4. Upgrade existing examples to schema `2026-09-26` with embedded dependencies and update workflow/compile/plugin documentation.
5. Replace fixed-bundle integration assertions with externally declared node fixtures, including a crate registering multiple kinds.

Rollback consists of retaining the previous CLI release and its source-checkout workflow. Retain old definitions for rollback, or downgrade their version and remove `dependencies` when using the previous CLI and its fixed bundle. The new lock can remain unused; runtime output format is unchanged. Package publication remains a separate maintainer action; the release workflow checks ownership and package availability before distributing the CLI.
