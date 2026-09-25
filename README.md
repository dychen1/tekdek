# Tekdek

## Overview

- Purpose: help engineers follow one behavior of a codebase while they keep the file structure in view.
- Scope: Python and Rust indexing, agent-written flows with clusters, the Lift Deck viewer, release installers, and a Claude Code plugin.
- Approach: index source facts into a graph, let a coding agent write a flow against that graph, freeze the flow, and play it in the browser.
- Boundaries: a flow explains ordered visits to functions; it never changes source facts. Lines show static calls in the source, not runtime counts.
- Outcome: a frozen, shareable flow that plays the same way for every reader.

## Requirements

- Put a language server on `PATH` for each language that you index: `rust-analyzer` for Rust and `pyright-langserver` for Python. Use a language-qualified `--server` option to select another executable.
- Run `tekdek doctor` to check the servers. It prints the install command of each missing server.
- To build from source, install the stable Rust toolchain and Node.js 22 or later. The build bundles the viewer with the locked npm dependencies.

## Install

Install the `tekdek` binary with one of these methods:

- macOS and Linux:

  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dychen1/tekdek/releases/latest/download/tekdek-installer.sh | sh
  ```

- Windows (PowerShell):

  ```powershell
  powershell -ExecutionPolicy Bypass -c "irm https://github.com/dychen1/tekdek/releases/latest/download/tekdek-installer.ps1 | iex"
  ```

- From source:

  ```sh
  git clone https://github.com/dychen1/tekdek && cd tekdek
  npm ci
  cargo install --locked --path .
  ```

The installers download from GitHub Releases. While `dychen1/tekdek` is a private repository, they work only after the repository becomes public. Until then, download a release archive with `gh release download --repo dychen1/tekdek`, or build from source.

Install the agent skills in Claude Code:

```text
/plugin marketplace add dychen1/tekdek
/plugin install tekdek@tekdek
```

The plugin adds `/tekdek:flow` and `/tekdek:flow-view`. It calls the `tekdek` binary, so install the binary first. For a private repository, Claude Code uses your git credentials (for GitHub, run `gh auth login` and `gh auth setup-git` once). Other coding agents can read `plugin/skills/flow/SKILL.md` and follow it.

## Quick Start

Open the demo from a checkout: a flow that explains how this tool freezes a flow, played against the graph of this repository.

```sh
tekdek serve --store demo --flow flow-generation --open
```

Press the right arrow to start the flow. Press `?` to show the key.

## Workflow

1. Index the codebase. Run this in the project root. It writes the graph to `.tekdek/<project>.json` and refreshes `.tekdek/index.json`.

   ```sh
   tekdek index
   ```

2. Ask a coding agent to write a flow:

   ```text
   /tekdek:flow explain how a request is authenticated
   ```

   The agent finds node keys with `tekdek graph search` and `tekdek graph show`, writes one agent output document with clusters and a label on every step, checks it, and freezes it:

   ```sh
   tekdek flow import request-auth --graph my-project.json --input output.json --check
   tekdek flow import request-auth --graph my-project.json --input output.json --author-name claude-code
   tekdek flow verify request-auth
   ```

3. View the flow:

   ```text
   /tekdek:flow-view request-auth
   ```

   Or run `tekdek serve --flow request-auth --open`. The server runs until you stop it.

Every command after `index` uses the nearest `.tekdek` folder from the working directory. Pass `--store <directory>` to use another store.

## Project Store

`tekdek index` writes to the nearest `.tekdek` folder in the codebase root or its parents. If there is none, it creates `<root>/.tekdek` with a `.gitignore` for the local lock file and interrupted writes.

| Path | Content |
| --- | --- |
| `<name>.json` | The graph of one indexed codebase. |
| `index.json` | The catalog that the viewer reads: graphs, digests, and current flows. |
| `flows/<name>/` | The frozen generations of one flow and its `current.json` pointer. |

Commit the `.tekdek` folder to share flows. A flow binds to the digest of its graph, so commit the graph file with its flows: `flow verify` and the viewer need that graph. When you index changed source again, the graph digest changes and the flows become stale. A stale flow still plays, with a warning. To follow the new graph, write the flow again under a new name.

## Lift Deck

The viewer shows the whole codebase as one vertical file tree. Directories and files start closed. When a flow starts, the tree opens the path to every function that the flow needs, once. The tree does not move again until you clear the flow.

Each flow cluster lifts its functions out of the tree as boxes onto a raised colored deck. Dashed slots and tethers join each box to its place in the tree. The callout at right describes the current cluster. The extracted stack lists the lifted functions in step order. The controls at the bottom show the step title and its annotation.

| Key | Action |
| --- | --- |
| Left and Right | Previous or next step. |
| Esc | Close the key, then clear the flow. |
| F | Fit the map. |
| ? | Show or hide the key. |

The URL keeps the codebase, the flow, and the step: `?artifact=<file>&flow=<name>&step=<n>`. A flow whose graph changed after the freeze plays with a stale warning.

`tekdek serve` binds to the loopback address. The viewer page, script, stylesheet, and fonts are embedded in the binary. The server reads only the store files that `index.json` lists.

## Frozen Flows

A flow is one frozen, digest-bound artifact for each generation. The coding agent names 1 to 8 clusters for the phases of the flow and labels every step with one cluster. Code derives function membership (all steps of one function carry the same cluster), cluster colors, visit runs, static call transitions, step IDs, and digests. The same agent output on the same graph always freezes the same bytes, except provenance.

`flow agent-contract` prints the JSON Schema of the agent output. `flow import` validates the complete document and freezes one generation. On any error it prints every issue and writes nothing. `flow import --check` does the complete import and writes nothing.

The store decides the generation purpose:

- A new name freezes an `initial` generation. The output has no prior step IDs and no revision summary.
- A name with a frozen generation freezes a `revision` with a diff against its parent. Set `priorStepId` on the steps that continue a prior step, and write a revision summary. The graph must be the graph of the parent; to follow a changed graph, import the flow under a new name.

Generations live in `<store>/flows/<name>/generations/<ordinal>-<digest12>.json`. `<store>/flows/<name>/current.json` names the current one. The catalog lists it under its graph. `flow verify` recomputes the derived layers and digests; an edited generation fails with `digest_mismatch`.

## Language Analysis

The source file extensions select the language adapters:

- `.py` and `.pyi` select Python.
- `.rs` selects Rust.

The analyzer starts a language server only for a detected and enabled language. It runs the selected adapters in stable language order. All selected adapters must succeed. If one adapter fails, the analyzer writes no artifact.

### Python

The Python adapter analyzes `.py` and `.pyi` documents. It prefers a `.py` file when a matching `.pyi` file exists. It uses the RustPython parser for syntax facts (modules, classes, callables, fields, containment, internal imports, signatures, documentation, and source ranges) and Pyright through LSP for semantic facts (calls, references, and type definitions).

The graph builder matches LSP locations to syntax declarations and scopes. Symbols outside the selected source root do not create graph edges.

### Rust

The Rust adapter analyzes `.rs` documents. It uses `syn` for syntax facts and rust-analyzer for semantic facts.

The Rust parser does not expand macros. It does not evaluate conditional compilation. A generated symbol can have no matching syntax node. The analyzer skips such a Rust semantic anchor.

The default rust-analyzer profile disables Cargo build scripts, procedural macros, check on save, and cache priming. A user configuration file can enable these features.

### Language Boundaries

Each graph node contains its language. The graph metadata lists all analyzed languages. Semantic edges connect nodes from the same language only.

## CLI Reference

```text
tekdek index [<root>] [--name <name>] [--store <directory>] | [--output <path>]
  [--config <path>]
  [--server <language>=<program>]
  [--server-arg <language>=<argument>]...

tekdek graph search <text> [--kind module|type|callable|field] [--limit <n>]
tekdek graph show <key>

tekdek flow list [--graph <file>]
tekdek flow show <name>
tekdek flow import <name> --graph <file> --input <path or -> [--check]
  [--author agent|human|system] [--author-name <name>] [--model-id <id>]
tekdek flow verify <name>
tekdek flow agent-contract

tekdek serve [--store <directory>] [--port <n>] [--flow <name>] [--open]
tekdek doctor [--config <path>] [--json]
tekdek help
tekdek version
```

The `graph` and `flow` commands accept `--store <directory>` and `--json`, and `graph` also accepts `--graph <file>`. The default store is the nearest `.tekdek` folder from the working directory. A `--graph` value is a JSON file name inside the store; the default graph of `graph` commands is the index default. Use `--input -` to read the agent output from standard input, and `--option=value` when a value starts with a hyphen. JSON mode returns one document, including for errors.

| Command or argument | Description |
| --- | --- |
| `index [<root>]` | Indexes the codebase root. The default root is the working directory. |
| `--name <name>` | Writes `<store>/<name>.json` and refreshes `<store>/index.json`. The default name is the root folder name. |
| `--store <directory>` | Selects the store. The default is the nearest `.tekdek` folder, or a new `<root>/.tekdek` for `index`. |
| `--output <path>` | Writes one artifact and leaves every index as it is. It cannot appear with `--name` or `--store`. |
| `--config <path>` | Reads profile overrides from this TOML file. |
| `--server <language>=<program>` | Replaces one language server program. |
| `--server-arg <language>=<argument>` | Replaces one profile's server arguments. Repeat this option to add arguments in order. |
| `graph search` | Lists the node keys that contain every word of the text, best name match first, with the kind and signature of each. The default limit is 20. |
| `graph show` | Shows one node: its signature, documentation summary, container, members, callers, and callees. A missing key names close keys. |
| `flow list` | Lists the current generation of each flow, with its freshness. |
| `flow show` | Shows the current generation: clusters, then steps. JSON mode prints the artifact. |
| `flow import` | Freezes one agent output. Returns 1 for a `no_flow` answer and 2 for a rejected output. The default author is `system`; `--author-name` alone records an agent. `--check` writes nothing. |
| `flow verify` | Recomputes the current generation. Returns 1 when it finds an issue. |
| `flow agent-contract` | Prints the agent output JSON Schema. |
| `serve` | Serves the Lift Deck on `127.0.0.1` and prints the URL. The default port is 8000; port 0 selects a free port. `--flow` prints the URL of one flow, and `--open` opens the URL in a browser. |
| `doctor` | Prints the version or the install command of each language server. Returns 1 when an enabled language has no server. |

Python uses `pyright-langserver --stdio` by default. Rust uses `rust-analyzer` by default.

## Configuration

The analyzer discovers only the global TOML configuration. It does not discover configuration in the analyzed repository.

The default path is `tekdek/config.toml` in the operating system configuration directory. On macOS, this is usually `~/Library/Application Support/tekdek/config.toml`.

Use `--config <path>` to select another file. The analyzer trusts an explicit path. The path can point anywhere, including into the analyzed repository. Inspect an explicit configuration file before you use it.

Each `[languages.<name>]` table can set these fields:

- `enabled` enables or disables the profile.
- `extensions` replaces the source extensions.
- `language_id` replaces the LSP language identifier.
- `program` replaces the server program.
- `arguments` replaces all server arguments.
- `initialization_options` replaces the complete built-in initialization options object.

For example:

```toml
[languages.python]
enabled = true
program = "pyright-langserver"
arguments = ["--stdio"]

[languages.rust]
enabled = true
program = "rust-analyzer"
arguments = []

[languages.rust.initialization_options]
checkOnSave = false

[languages.rust.initialization_options.cargo.buildScripts]
enable = false

[languages.rust.initialization_options.procMacro]
enable = false

[languages.rust.initialization_options.cachePriming]
enable = false
```

The Rust initialization options in this example keep all built-in safety settings. Include each required safety setting when you replace this object.

Built-in settings have the lowest priority. The global configuration overrides built-in settings. CLI options override the global configuration.

Program-only CLI overrides keep the profile arguments. One or more `--server-arg` options replace the profile arguments.

```sh
tekdek index . \
  --server rust=/opt/rust-analyzer \
  --server-arg rust=--log-file \
  --server-arg rust=/tmp/rust-analyzer.log
```

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
|   |-- cli/                   # Command routing and the index, graph, flow, serve, and doctor commands
|   |-- analysis/              # Indexer: pipeline, preflight, configuration, sources
|   |   |-- fact/              # Syntax and semantic fact models
|   |   |-- graph/             # Graph models and construction (build, semantic, lookup)
|   |   |-- language/          # Python and Rust adapters and their profiles
|   |   `-- lsp/               # Language server client, session, and capture
|   |-- store/                 # Store folder, project store, atomic writes, graph artifact
|   |-- flow/                  # Frozen flows: models, import, verify, lineage, rules
|   |   `-- schema/            # Agent output and flow artifact JSON Schemas
|   |-- catalog.rs             # index.json for the viewer
|   |-- query.rs               # Graph search and node neighbors for agents
|   `-- serve.rs               # Viewer server with the embedded viewer
|-- tools/build-viewer.mjs     # esbuild script that build.rs runs
|-- viewer/
|   |-- index.html             # Lift Deck page
|   |-- src/                   # Strict TypeScript: data, layout, deck
|   |-- styles/deck.css        # Tokens and styles
|   |-- fonts/                 # JetBrains Mono latin subset (SIL OFL 1.1)
|   `-- tests/                 # Viewer tests
|-- demo/                      # Demo store: this repository's graph and one flow
|-- .claude-plugin/            # Claude Code marketplace
|-- plugin/                    # Claude Code plugin: the flow and flow-view skills
`-- .github/                   # Release workflow and its build setup steps
```

The analysis runs in this order:

```text
codebase root -> source documents -> syntax facts -> semantic facts -> graph -> graph artifact
```

The language adapter produces syntax facts from the source syntax and semantic facts from a language server. The syntax facts define all graph nodes. The graph builder resolves the semantic facts against the syntax nodes and adds call, reference, and type-use edges.

## Development

```sh
npm ci
make check
```

`make check` runs `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features`, the TypeScript check, `cargo test --locked --all-targets`, and `npm test`. `make test` runs the two test suites only. `make demo` serves the demo store, `make serve` serves the nearest `.tekdek` folder (set `STORE=<directory>` for another store), and `make index SOURCE=<root>` indexes a codebase. Set `PORT=8001` to use another port.

Every Cargo build runs `build.rs`, which bundles `viewer/src` into the build output with esbuild. A build without `npm ci` fails with a message that says to run it.

## Releases

1. Set the same version in `Cargo.toml` and `plugin/.claude-plugin/plugin.json`. A test fails when they differ.
2. Merge the change to `main`.
3. Tag the commit on `main` and push the tag:

   ```sh
   git tag v0.2.0 && git push origin v0.2.0
   ```

The release workflow builds archives for macOS (arm64 and x86_64), Linux (x86_64 and arm64), and Windows (x86_64), with shell and PowerShell installers, and publishes them on GitHub Releases. It stops when the tag does not point to a commit on `main`. Claude Code users receive the plugin update when the plugin version changes.
