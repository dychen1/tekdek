<p align="center">
  <img src=".github/banner.svg" alt="tekdek: a fingerboard deck whose graphic shows three functions lifted out of a file tree onto a deck" width="100%">
</p>

# Tekdek

## Overview

- Purpose: help engineers follow one behavior of a codebase while the file structure stays in view.
- Scope: Rust, Python, TypeScript, and JavaScript indexing, agent-written flows with clusters, the Lift Deck viewer, installers, and a Claude Code plugin.
- Approach: index the source into a graph, let a coding agent write a flow against the graph, freeze the flow, and play it in the browser.
- Boundaries: a flow explains ordered visits to functions and never changes the source facts. Lines show static calls, not runtime counts.
- Outcome: a frozen, shareable flow that plays the same way for every reader.

## Install

- macOS and Linux:

  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dychen1/tekdek/releases/latest/download/tekdek-installer.sh | sh
  ```

- Windows (PowerShell):

  ```powershell
  powershell -ExecutionPolicy Bypass -c "irm https://github.com/dychen1/tekdek/releases/latest/download/tekdek-installer.ps1 | iex"
  ```

- From source, with rustup and Node.js 22 or later. rustup installs the Rust version that `rust-toolchain.toml` pins:

  ```sh
  git clone https://github.com/dychen1/tekdek && cd tekdek
  npm ci
  cargo install --locked --path .
  ```

The installers need no Rust or Node.js. They put `tekdek` in `$XDG_BIN_HOME`, else `$XDG_DATA_HOME/../bin`, else `~/.local/bin`, and add that folder to `PATH`. The Linux binaries need glibc 2.34 or later: Ubuntu 22.04, Debian 12, RHEL 9, Amazon Linux 2023, or newer. Alpine and older distributions cannot run them; build from source there.

The installers download from GitHub Releases without credentials, so they work only after the repository becomes public. Until then, a user with access can download the archive for their system with the GitHub CLI, extract it, and move the binary into a folder on `PATH`:

```sh
gh release download --repo dychen1/tekdek --pattern 'tekdek-aarch64-apple-darwin.tar.xz'
tar -xJf tekdek-aarch64-apple-darwin.tar.xz
mkdir -p ~/.local/bin && mv tekdek-aarch64-apple-darwin/tekdek ~/.local/bin/
```

The archives are `tekdek-aarch64-apple-darwin.tar.xz` (Apple silicon), `tekdek-x86_64-apple-darwin.tar.xz` (Intel Mac), `tekdek-x86_64-unknown-linux-gnu.tar.xz`, `tekdek-aarch64-unknown-linux-gnu.tar.xz`, and `tekdek-x86_64-pc-windows-msvc.zip`. Add `--tag v0.1.0` for a specific release.

Install the agent skills in Claude Code:

```text
/plugin marketplace add dychen1/tekdek
/plugin install tekdek@tekdek
```

The plugin adds `/tekdek:flow` and `/tekdek:flow-view`, which call the `tekdek` binary. For the private repository, Claude Code uses your git credentials (`gh auth login`, then `gh auth setup-git`). Other coding agents can follow `plugin/skills/flow/SKILL.md`.

## Language Servers

`tekdek index` parses the source and starts no language server. `tekdek graph show`, `tekdek flow import`, and `tekdek flow verify` get calls, references, and types from a language server, only for the nodes that they read (see [Semantic Edges](#semantic-edges)). The other commands need no language server.

| Language | Indexed | Server | Install |
| --- | --- | --- | --- |
| Rust | Yes | `rust-analyzer` | `rustup component add rust-analyzer`, `brew install rust-analyzer`, or a [release binary](https://github.com/rust-lang/rust-analyzer/releases) |
| Python | Yes | `pyright-langserver` | `npm install -g pyright`, or without Node.js: `uv tool install 'pyright[nodejs]'` or `pipx install 'pyright[nodejs]'` |
| TypeScript and JavaScript | Yes | `tsc --lsp` of TypeScript 7 | `npm install -g typescript` |
| Go | Not yet | `gopls` | `go install golang.org/x/tools/gopls@latest` |

- `tekdek doctor` prints the path and version of each server, or the install command of a missing server. It runs the version command of each server, so a server that cannot run, such as a rustup proxy without the component, is `broken`. pyright reports its version through the `pyright` program next to `pyright-langserver`; without it, the version is unverified.
- `tekdek doctor <root>` checks only the languages that have source files in the root. A Rust project then does not need pyright.
- Name a server that is not on `PATH` in the [configuration file](#configuration), for example `program = ".venv/bin/pyright-langserver"` in `[languages.python]`.
- The TypeScript server is the `tsc` program of TypeScript 7 or later. Earlier versions have no `--lsp` option. To use the TypeScript of a project, set `program` in `[languages.typescript]` to the absolute path of its `node_modules/.bin/tsc`.
- The TypeScript server resolves ES module imports in TypeScript and JavaScript. It does not resolve `require` calls, so a CommonJS call into another file gets no edge.
- `tekdek index` skips the files of a language that it does not index yet. Request a language in [issue #4](https://github.com/dychen1/tekdek/issues/4).
- `/tekdek:flow` looks for a local server first and asks before it installs one, for example `pyright needed for project acme-api - ~20 MB. y or n to download and install?`.

## Quick Start

Open the demo from a checkout: a flow that explains how this tool freezes a flow, played against the graph of this repository.

```sh
tekdek serve --store demo --flow flow-generation --open
```

Press the right arrow to start the flow. Press `?` to show the key.

## Workflow

1. Index the project: run `tekdek index` in the project root. It parses the source, so it takes seconds or less.
2. Ask a coding agent for a flow, for example `/tekdek:flow explain how a request is authenticated`. The agent finds node keys with `tekdek graph search` and `tekdek graph show`, writes an output document with clusters, checks it with `tekdek flow import --check`, and freezes it with `tekdek flow import`.
3. View the flow with `/tekdek:flow-view request-auth` or `tekdek serve --flow request-auth --open`.

Every command uses the nearest `.tekdek` folder from the working directory. The search stops at the top folder of the Git work tree, and it does not go up into your home folder. Pass `--store <directory>` to use another store.

## Project Store

`tekdek index` writes to the nearest `.tekdek` folder at or above the root. When the project has no store, `index` creates `<root>/.tekdek` after the analysis succeeds. Commit the folder to share flows.

- In a project that has a store, `tekdek index` without a root indexes the project root from any subfolder, as Git commands do. The project root is the folder that holds the store.
- A root argument inside that project needs `--name`, so a partial graph never replaces the project graph: `tekdek index services/worker --name worker` writes `worker.json`.

```text
acme-api/
|-- src/
`-- .tekdek/
    |-- .gitignore                      # ignores the lock files, the daemon files, interrupted writes, and index.json
    |-- acme-api.json                   # graph of the project, from tekdek index
    |-- acme-api.semantic.json          # semantic edges of the nodes that commands read
    `-- flows/
        |-- request-auth/
        |   |-- current.json            # names the current generation
        |   `-- generations/
        |       |-- 1-3f9a0c1d2e4b.json # first freeze
        |       `-- 2-8b21d0e7aa13.json # revision of 1, now current
        `-- billing-webhook/
            |-- current.json
            `-- generations/
                `-- 1-c0d4e5f60718.json
```

- A flow is one folder. Each freeze adds one generation file, named by its ordinal and the first 12 characters of its digest. A generation never changes after the freeze. `current.json` names the current generation.
- The graph belongs to the store, not to one flow. All flows of a project share its graph.
- The catalog of graphs and current flows is not a file in the store. `tekdek serve` builds it from the store for each request, so two branches that add flows merge without a conflict. An `index.json` file from an older tekdek has no effect; delete it.
- The default graph is the only graph, else the graph named after the project folder, else the first graph in name order. A named index does not change the default. The `graph` commands and the viewer use the default when you do not name a graph.
- A flow binds to the digest of its graph, so commit the graph with its flows. When you index changed source again, the digest changes and the flows become stale. A stale flow still plays, with a warning. To follow the new graph, write the flow again under a new name.
- Commit `<graph>.semantic.json` with its graph, so `flow verify` runs without a language server, for example in CI. `graph show` adds to it, so expect changes in this file.
- A project folder named `x.semantic` needs `tekdek index --name <name>`, because `x.semantic.json` is the name of a semantic cache.

## Frozen Flows

The agent names 1 to 8 clusters for the phases of a flow and labels each step with one cluster. Code derives the rest: function membership, cluster colors, visit runs, static call transitions, step IDs, and digests. The same agent output on the same graph always freezes the same bytes, except the provenance.

- `flow agent-contract` prints the JSON Schema of the agent output.
- `flow import` validates the complete output and freezes one generation. On any error, it prints every issue and writes no flow. `--check` runs the complete import and writes no flow. Both can add the edges of the step nodes to the semantic cache (see [Semantic Edges](#semantic-edges)).
- A new name freezes an `initial` generation. An existing name freezes a `revision` with a diff against its parent, on the graph of the parent.
- `flow verify` recomputes the derived layers and digests. An edited generation fails with `digest_mismatch`.

## Semantic Edges

`tekdek index` writes the syntax graph: the nodes and their `contains` edges. The call, reference, type, and import edges come later, from a language server, for the nodes that a command reads.

- `graph show <key>` gets the edges of its node. `flow import` and `flow verify` get the edges of every step node before they derive the fact layer.
- A daemon of the store keeps the servers warm. The first command starts it with the servers, which takes some seconds. The next commands take milliseconds for each node. The daemon stops after 10 minutes without a request. It keeps the search path of the command that started it: when a server program is not on that path, it answers the error and stops, so the next command starts a daemon with its own path. Its files are `daemon.lock`, `daemon.sock`, and `daemon.log` in the store; `index` and the daemon add them to the `.gitignore` of an older store.
- When `flow verify` cannot get the edges, for example in CI without a server, it prints a warning and verifies against the graph and its cache as they are.
- The edges go to `<graph>.semantic.json`, which records the byte digest of its graph. After a new index, the next command replaces the cache. The cache is not part of the flow binding, so it never makes a flow stale. `serve` sends each graph with the edges of its cache.
- Only a graph of the complete project folder, the folder that holds the store, gets semantic edges. For a graph of a subfolder, or of another root, these commands fail.
- `TEKDEK_NO_DAEMON=1` resolves in the command process: each command starts and stops its servers. `TEKDEK_DAEMON_IDLE_SECONDS=<n>` sets the idle time of the daemon. `TEKDEK_TIMINGS=1` prints the time of each stage to standard error.

Known limits:

- A Rust callee in a macro, or a function passed as a value, shows as a callee only after a `graph show` of that callee.
- The signature types and the value uses of a node show when the other end is resolved too. The edges between the steps of a flow are complete, because a flow resolves all its steps.
- rust-analyzer can miss some callers of trait methods.

## Lift Deck

The viewer has five parts. The explorer at left shows the files of the codebase. The current file's functions branch out of the explorer into a file box. The current cluster's functions sit on a pinned deck, and the current step's function is tethered to its slot in the file box. The code column at right shows the step title, its annotation, the transition from the step before, the callers and callees, and the whole function source. The bottom bar shows the cluster description and a strip of every step.

| Key | Action |
| --- | --- |
| K or Right | Next step |
| J or Left | Previous step |
| F | Show the code full screen, or return |
| C | Fold or name the functions that are not in the flow |
| Esc | Close the key, then leave full screen, then clear the flow |
| ? | Show or hide the key |

The URL keeps the graph, the flow, and the step: `?artifact=<file>&flow=<name>&step=<n>`. `tekdek serve` binds to the loopback address, serves the viewer from the binary, and reads only the store files that its catalog lists. It also reads the source files of a flow's step functions, inside the project folder that holds the store. A function whose source changed after the last index shows a note in place of its source. Before it serves a `.tekdek` store, `tekdek serve` reindexes the project graph if the source changed after the last index, the same as `tekdek index` with no options. `--no-index` skips that step. `tekdek serve` sends each graph with the semantic edges in its cache, and starts no language server.

## Language Analysis

- The file extension selects the adapter: `.rs` for Rust, `.py` and `.pyi` for Python, and `.ts`, `.tsx`, `.mts`, `.cts`, `.js`, `.jsx`, `.mjs`, and `.cjs` for TypeScript and JavaScript. A `.py` file wins over a matching `.pyi` file. TypeScript declaration files, such as `index.d.ts`, have no node.
- `index` skips hidden folders, and folders named `node_modules`, `target`, or `__pycache__`. For TypeScript and JavaScript, it also skips folders named `dist`, `build`, or `coverage`.
- A TypeScript or JavaScript file that does not parse keeps its file node without declarations, and `index` prints a warning that names it. A Python or Rust file that does not parse stops the index.
- A parser makes the syntax facts, which define all graph nodes: `syn` for Rust, the RustPython parser for Python, and the Oxc parser for TypeScript and JavaScript. A language server adds the call, reference, and type edges of the nodes that a command reads.
- Edges connect nodes of the same language only. Symbols outside the source root make no edges.
- A TypeScript or JavaScript function is a node when a declaration, a class member, or a variable names it: `function run()`, `run() {}` in a class, and `const run = () => {}`. A callback or an object method belongs to the function or file that holds it. Overload signatures and their implementation are one node.
- The Rust parser does not expand macros or evaluate conditional compilation. The default rust-analyzer profile disables build scripts, procedural macros, check on save, and cache priming.
- When one adapter fails, `index` writes no graph.

## CLI Reference

```text
tekdek index [<root>] [--name <name>] [--store <directory>] | [--output <path>]
  [--config <path>]

tekdek graph search <text> [--kind module|type|callable|field] [--limit <n>]
tekdek graph show <key> [--config <path>]

tekdek flow list [--graph <file>]
tekdek flow show <name>
tekdek flow import <name> --graph <file> --input <path or -> [--check] [--config <path>]
  [--author agent|human|system] [--author-name <name>] [--model-id <id>]
tekdek flow verify <name> [--config <path>]
tekdek flow agent-contract

tekdek serve [--store <directory>] [--port <n>] [--flow <name>] [--open] [--no-index]
tekdek doctor [<root>] [--config <path>] [--json]
tekdek help
tekdek version
```

The `graph` and `flow` commands accept `--store <directory>` and `--json`. `--graph` takes a file name inside the store. Write `--option=value` for a value that starts with a hyphen.

Every command uses the same exit codes:

| Code | Meaning |
| --- | --- |
| 0 | The command succeeded. |
| 1 | A check found a problem: `flow verify` found an issue, `flow import` got a `no_flow` answer, or `doctor` found a needed server that is missing or broken. |
| 2 | An error: an invalid command line, a missing store or graph, a rejected agent output, a failed analysis, or a failed language server. |

With `--json`, the result or the error is one JSON document on standard output. Without it, results go to standard output and errors go to standard error.

| Command | Notes |
| --- | --- |
| `index` | The default root is the working directory, or the project root when a store exists. The default name is the root folder name. A root inside a project with a store needs `--name`. `--output` writes one graph and changes no store. |
| `graph search` | Lists the keys that contain every word, best name match first. Spaces, `::`, `:`, `.`, and `/` separate words, so `Command::parse` and `flow/import` work. The default limit is 20. JSON output names the graph file. |
| `graph show` | Shows the signature, documentation summary, container, members, callers, and callees. A missing key names close keys. It reindexes the project graph if the source changed, then gets the edges of the node (see [Semantic Edges](#semantic-edges)). |
| `flow list` | Lists the current generation of each flow, with its freshness. |
| `flow import` | `--author-name` alone records an agent author. Gets the edges of the step nodes first. |
| `flow verify` | Recomputes the derived layers and digests of the current generation. Gets the edges of the step nodes first, unless the flow is stale. |
| `serve` | Serves on `127.0.0.1:8000` by default. Port 0 selects a free port. `--flow` prints the URL of one flow, and `--open` opens it. |
| `doctor` | With a root, checks only the languages of the root. Statuses: `ok`, `broken`, `missing`, `disabled`, and `not used`. |

## Configuration

tekdek reads `tekdek/config.toml` in the configuration directory of the operating system (on macOS, `~/Library/Application Support/tekdek/config.toml`), or the file that `--config <path>` names. It never reads a configuration from the analyzed repository. Inspect a `--config` file before you use it.

Each `[languages.<name>]` table can set `enabled`, `extensions`, `language_id`, `program`, `arguments`, and `initialization_options`. The file overrides the built-in settings. Give `index`, `graph show`, `flow import`, and `flow verify` the same `--config` file: the enabled languages and extensions select the source files that the graph records.

```toml
[languages.python]
program = "pyright-langserver"
arguments = ["--stdio"]

[languages.rust.initialization_options]
checkOnSave = false
cargo.buildScripts.enable = false
procMacro.enable = false
cachePriming.enable = false
```

`initialization_options` replaces the complete built-in object. For Rust, keep the four safety settings of this example.

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
|   |-- analysis/              # Indexer: pipeline, semantic resolver, preflight, program lookup, configuration, sources
|   |   |-- fact/              # Syntax and semantic fact models
|   |   |-- graph/             # Graph models and construction
|   |   |-- language/          # Python, Rust, and TypeScript adapters and their profiles
|   |   `-- lsp/               # Language server client, session, and capture
|   |-- store/                 # Store folder, project store, atomic writes, graph artifact, semantic cache
|   |-- flow/                  # Frozen flows: models, import, verify, lineage, rules
|   |   `-- schema/            # Agent output and flow artifact JSON Schemas
|   |-- catalog.rs             # Catalog of graphs and flows for the viewer
|   |-- query.rs               # Graph search and node neighbors for agents
|   |-- semantic.rs            # Semantic edges on demand; semantic/daemon.rs keeps the servers warm
|   |-- serve.rs               # Viewer server with the embedded viewer
|   |-- serve/files.rs         # Store and project file reads that follow no link
|   `-- source_view.rs         # Function source cuts for the code column of the viewer
|-- tools/build-viewer.ts      # esbuild script that build.rs runs
|-- viewer/
|   |-- index.html             # Lift Deck page
|   |-- src/                   # Strict TypeScript: data, layout, deck
|   |-- styles/deck.css        # Tokens and styles
|   |-- fonts/                 # JetBrains Mono latin subset (SIL OFL 1.1)
|   `-- tests/                 # Viewer tests
|-- demo/                      # Demo store: this repository's graph and one flow
|-- .claude-plugin/            # Claude Code marketplace
|-- plugin/                    # Claude Code plugin: the flow and flow-view skills
`-- .github/                   # CI and release workflows, and the release build setup steps
```

## Development

```sh
npm ci
make check
```

`make check` runs `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features`, the TypeScript check, `cargo test --locked --all-targets`, and `npm test`. `make test` runs the two test suites only. `make demo` serves the demo store, `make serve` serves the nearest `.tekdek` folder (`STORE=<directory>` for another store), and `make index SOURCE=<root>` indexes a codebase. `PORT=8001` selects another port.

Every Cargo build runs `build.rs`, which bundles `viewer/src` with esbuild. A build without `npm ci` fails with a message that says to run it.

## Releases

The CI workflow runs `make check` on every pull request and every push to `main`.

1. Set the new version in `Cargo.toml`, `plugin/.claude-plugin/plugin.json`, and the "needs tekdek X or later" line of both skills. Tests fail when they differ.
2. Merge the change to `main`, and wait until its CI run passes.
3. Tag that commit and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.
4. Wait until the release workflow succeeds and the release has its archives and installers.

The release workflow builds archives and installers for macOS (arm64 and x86_64), Linux (x86_64 and arm64), and Windows (x86_64), and publishes them on GitHub Releases. Each archive holds the binary, `LICENSE`, `README.md`, and `OFL.txt`, the license of the embedded font. A build job stops when the tag does not point to a commit on `main`. That check only catches mistakes: the workflow runs from the tagged commit, so a tag on another commit can change the check, and a free private repository has no tag rules. Tag only `main` commits whose CI run passed. Claude Code users get the plugin update when the plugin version changes.

The crate sets `publish = false`: `build.rs` needs the npm dependencies of a checkout, so a crate from crates.io could not build.
