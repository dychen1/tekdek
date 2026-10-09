<p align="center">
  <img src=".github/banner.svg" alt="tekdek: a fingerboard deck whose graphic shows three functions lifted out of a file tree onto a deck" width="100%">
</p>

# Tekdek

## Overview

- Purpose: help engineers follow one behavior of a codebase while they keep the file structure in view.
- Scope: Python and Rust indexing, agent-written flows with clusters, and the Lift Deck viewer.
- Approach: index source facts into a graph, let a coding agent write a flow against that graph, freeze the flow, and play it in the browser.
- Boundaries: a flow explains ordered visits to functions; it never changes source facts. Lines show static calls in the source, not runtime counts.
- Outcome: a frozen, shareable flow that plays the same way for every reader.

## Requirements

- Install the stable Rust toolchain and Cargo to build the analyzer.
- Put `pyright-langserver` on `PATH` to analyze Python.
- Put `rust-analyzer` on `PATH` to analyze Rust.
- Use a language-qualified `--server` option to select another executable.
- Install Node.js 22 or later to run the viewer server and the viewer tests.

## Install

Build the CLI and put `tekdek` on your `PATH`:

```sh
cargo install --locked --path .
npm ci
```

The viewer runs from this checkout: `make serve` and `make demo` build the browser bundle and start the local server.

## Quick Start

Open the demo: a flow that explains how this tool freezes a flow, played against the graph of this repository.

```sh
make demo
```

Open `http://127.0.0.1:8000/viewer/`. Press the right arrow to start the flow. Press `?` to show the key.

## Workflow

1. Index a codebase. The command writes `artifacts/<name>.json` and refreshes `artifacts/index.json`.

   ```sh
   make index SOURCE=/path/to/codebase NAME=my-project
   ```

2. Ask a coding agent to write a flow. The agent follows `.skills/flow/SKILL.md`: it finds node keys in the graph, writes one agent output document with clusters and a label on every step, and freezes it.

   ```sh
   tekdek flow import request-handling --graph my-project.json \
     --input output.json --author-name claude-code
   tekdek flow verify request-handling
   ```

3. View the flow.

   ```sh
   make serve
   ```

   Open `http://127.0.0.1:8000/viewer/`, select the codebase and the flow, and step through it.

Set `ARTIFACT_STORE=/path/to/store` to use another store and `PORT=8001` to use another port.

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

The viewer server binds to the loopback address. It serves the viewer and the files that `index.json` lists.

## Frozen Flows

A flow is one frozen, digest-bound artifact for each generation. The coding agent names 1 to 8 clusters for the phases of the flow and labels every step with one cluster. Code derives function membership (all steps of one function carry the same cluster), cluster colors, visit runs, static call transitions, step IDs, and digests. The same agent output on the same graph always freezes the same bytes, except provenance.

`flow agent-contract` prints the JSON Schema of the agent output. `flow import` validates the complete document and freezes one generation. On any error it prints every issue and writes nothing.

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
tekdek <root> [--name <name>] [--store <directory>] | [--output <path>]
  [--config <path>]
  [--server <language>=<program>]
  [--server-arg <language>=<argument>]...

tekdek flow list [--graph <file>]
tekdek flow show <name>
tekdek flow import <name> --graph <file> --input <path or ->
  [--author agent|human|system] [--author-name <name>] [--model-id <id>]
tekdek flow verify <name>
tekdek flow agent-contract

tekdek help
```

All flow commands accept `--store <directory>` and `--json`. The default store is `artifacts`. The `--graph` value is a JSON file name inside the store. Use `--input -` to read the agent output from standard input, and `--option=value` when a value starts with a hyphen. JSON mode returns one document, including for errors.

| Command or argument | Description |
| --- | --- |
| `<root>` | Selects the codebase root. |
| `--name <name>` | Writes `<store>/<name>.json` and refreshes `<store>/index.json`. The default name is the root folder name. |
| `--store <directory>` | Selects the store of the indexed artifact. The default is `artifacts`. |
| `--output <path>` | Writes one artifact and leaves every index as it is. It cannot appear with `--name` or `--store`. |
| `--config <path>` | Reads profile overrides from this TOML file. |
| `--server <language>=<program>` | Replaces one language server program. |
| `--server-arg <language>=<argument>` | Replaces one profile's server arguments. Repeat this option to add arguments in order. |
| `flow list` | Lists the current generation of each flow, with its freshness. |
| `flow show` | Shows the current generation: clusters, then steps. JSON mode prints the artifact. |
| `flow import` | Freezes one agent output. Returns 1 for a `no_flow` answer and 2 for a rejected output. The default author is `system`; `--author-name` alone records an agent. |
| `flow verify` | Recomputes the current generation. Returns 1 when it finds an issue. |
| `flow agent-contract` | Prints the agent output JSON Schema. |

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
tekdek . \
  --server rust=/opt/rust-analyzer \
  --server-arg rust=--log-file \
  --server-arg rust=/tmp/rust-analyzer.log
```

## Project Structure

```text
.
|-- Cargo.toml                 # Crate metadata and dependencies
|-- Makefile                   # serve, demo, build, index, test, check
|-- src/
|   |-- main.rs                # Binary entry
|   |-- lib.rs                 # Library modules
|   |-- cli/                   # Command routing, index command, flow commands
|   |-- analysis/              # Indexer: pipeline, preflight, configuration, sources
|   |   |-- fact/              # Syntax and semantic fact models
|   |   |-- graph/             # Graph models and construction (build, semantic, lookup)
|   |   |-- language/          # Python and Rust adapters and their profiles
|   |   `-- lsp/               # Language server client, session, and capture
|   |-- store/                 # Store folder, atomic writes, graph artifact
|   |-- flow/                  # Frozen flows: models, import, verify, lineage, rules
|   |   `-- schema/            # Agent output and flow artifact JSON Schemas
|   `-- catalog.rs             # index.json for the viewer
|-- viewer/
|   |-- index.html             # Lift Deck page
|   |-- src/                   # Strict TypeScript: data, layout, deck
|   |-- styles/deck.css        # Tokens and styles
|   |-- fonts/                 # JetBrains Mono latin subset (SIL OFL 1.1)
|   |-- server.mjs             # Local viewer server
|   `-- dist/                  # Generated browser bundle (ignored)
|-- demo/                      # Demo store: this repository's graph and one flow
`-- .skills/flow/SKILL.md      # Flow authoring skill for coding agents
```

The analysis runs in this order:

```text
codebase root -> source documents -> syntax facts -> semantic facts -> graph -> graph artifact
```

The language adapter produces syntax facts from the source syntax and semantic facts from a language server. The syntax facts define all graph nodes. The graph builder resolves the semantic facts against the syntax nodes and adds call, reference, and type-use edges.

## Development

```sh
make check
```

`make check` runs `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features`, the TypeScript check, `cargo test --locked --all-targets`, and `npm test`. `make test` runs the two test suites only.
