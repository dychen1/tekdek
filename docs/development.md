# Development

## Overview

- Purpose: help a contributor find the code, run the checks, and make a release.
- Scope: the project structure, the build and test commands, and the release steps.
- Boundaries: every Cargo build needs the npm dependencies. The crate is not on crates.io.
- Outcome: a change that passes `make check`, and a release that matches the plugin.

## Project Structure

```text
.
|-- Cargo.toml                 # Crate metadata and dependencies
|-- build.rs                   # Bundles the viewer script for the binary
|-- dist-workspace.toml        # Release configuration for cargo-dist
|-- Makefile                   # serve, demo, build, index, test, check
|-- src/
|   |-- main.rs                # Binary entry
|   |-- lib.rs                 # Library modules
|   |-- cli/                   # Command routing and the index, graph, flow, diff, serve, and doctor commands
|   |-- analysis/              # Indexer: pipeline, semantic resolver, preflight, program lookup, configuration, sources
|   |   |-- fact/              # Syntax and semantic fact models
|   |   |-- graph/             # Graph models and construction
|   |   |-- language/          # Python, Rust, and TypeScript adapters and their profiles
|   |   `-- lsp/               # Language server client, session, and capture
|   |-- store/                 # Store folder, project store, atomic writes, graph artifact, semantic cache
|   |-- flow/                  # Frozen flows: models, import, verify, lineage, rules
|   |   `-- schema/            # Agent output and flow artifact JSON Schemas
|   |-- diff/                  # PR diffs: base worktree, pairs and notes, line diff, freeze, verify
|   |-- catalog.rs             # Catalog of graphs, flows, and diffs for the viewer
|   |-- query.rs               # Graph search and node neighbors for agents
|   |-- semantic.rs            # Semantic edges on demand; semantic/daemon.rs keeps the servers warm
|   |-- serve.rs               # Viewer server with the embedded viewer
|   |-- serve/files.rs         # Store and project file reads that follow no link
|   `-- source_view.rs         # Function source cuts for the code column of the viewer
|-- tools/build-viewer.ts      # esbuild script that build.rs runs
|-- viewer/
|   |-- index.html             # Lift Deck page
|   |-- src/                   # Strict TypeScript: data, layout, deck, diff
|   |-- styles/deck.css        # Tokens and styles
|   |-- fonts/                 # JetBrains Mono latin subset (SIL OFL 1.1)
|   `-- tests/                 # Viewer tests
|-- demo/                      # Demo store: this repository's graph and one flow
|-- docs/                      # Documentation pages, and the demo video in assets/
|-- .claude-plugin/            # Plugin marketplace for Claude Code and Codex
|-- plugin/                    # Plugin: the flow, flow-view, and diff skills, and the bin/tekdek launcher
`-- .github/                   # CI and release workflows, and the release build setup steps
```

## Checks

```sh
npm ci
make check
```

`make check` runs `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features`, the TypeScript check, `cargo test --locked --all-targets`, and `npm test`. `make test` runs the two test suites only. `make demo` serves the demo store, `make serve` serves the nearest `.tekdek` folder (`STORE=<directory>` for another store), and `make index SOURCE=<root>` indexes a codebase. `PORT=8001` selects another port.

Every Cargo build runs `build.rs`, which bundles `viewer/src` with esbuild. A build without `npm ci` fails with a message that says to run it.

## Releases

The CI workflow runs `make check` on every pull request and every push to `main`.

1. Set the new version in `Cargo.toml`, `plugin/.claude-plugin/plugin.json`, and the "needs tekdek X or later" line of every skill. Tests fail when they differ.
2. Merge the change to `main`, and wait until its CI run passes.
3. Tag that commit and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.
4. Wait until the release workflow succeeds and the release has its archives and installers.
5. Set `ref` in `.claude-plugin/marketplace.json` to the new tag and merge it to `main`. Plugin users get the new skills only after this step, so the skills never ask for a binary that has no release yet. A test fails when the pinned tag is newer than the crate version.

The release workflow builds archives and installers for macOS (arm64 and x86_64), Linux (x86_64 and arm64), and Windows (x86_64), and publishes them on GitHub Releases. Each archive holds the binary, `LICENSE`, `README.md`, and `OFL.txt`, the license of the embedded font. A build job stops when the tag does not point to a commit on `main`. That check only catches mistakes: the workflow runs from the tagged commit, so a tag on another commit can change the check, and the repository has no tag rules. Tag only `main` commits whose CI run passed.

The crate sets `publish = false`: `build.rs` needs the npm dependencies of a checkout, so a crate from crates.io could not build.
