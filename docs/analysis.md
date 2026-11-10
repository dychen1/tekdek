# Analysis

## Overview

- Purpose: describe what the graph of a project holds and where each part comes from.
- Scope: the files and declarations that `tekdek index` records for each language, and the semantic edges that language servers add.
- Boundaries: `index` parses the source only. A language server adds edges only for the nodes that a command reads.
- Outcome: a syntax graph in seconds, and call, reference, and type edges on demand.

## Language Analysis

- The file extension selects the adapter: `.rs` for Rust, `.py` and `.pyi` for Python, and `.ts`, `.tsx`, `.mts`, `.cts`, `.js`, `.jsx`, `.mjs`, and `.cjs` for TypeScript and JavaScript. A `.py` file wins over a matching `.pyi` file. TypeScript declaration files, such as `index.d.ts`, have no node.
- `index` skips hidden folders, and folders named `node_modules`, `target`, or `__pycache__`. For TypeScript and JavaScript, it also skips folders named `dist`, `build`, or `coverage`. It does not read `.gitignore`, so it indexes the source files in other ignored folders.
- A TypeScript or JavaScript file that does not parse keeps its file node without declarations, and `index` prints a warning that names it. A Python or Rust file that does not parse stops the index.
- A parser makes the syntax facts, which define all graph nodes: `syn` for Rust, the RustPython parser for Python, and the Oxc parser for TypeScript and JavaScript. A language server adds the call, reference, and type edges of the nodes that a command reads.
- Edges connect nodes of the same language only. Symbols outside the source root make no edges.
- A TypeScript or JavaScript function is a node when a declaration, a class member, or a variable names it: `function run()`, `run() {}` in a class, and `const run = () => {}`. A callback or an object method belongs to the function or file that holds it. Overload signatures and their implementation are one node.
- The Rust parser does not expand macros or evaluate conditional compilation. The default rust-analyzer profile disables build scripts, procedural macros, check on save, and cache priming.
- When one adapter fails, `index` writes no graph.

See [Language Servers](install.md#language-servers) for the server of each language.

## Semantic Edges

`tekdek index` writes the syntax graph: the nodes and their `contains` edges. The call, reference, type, and import edges come later, from a language server, for the nodes that a command reads.

- `graph show <key>...` gets the edges of its nodes in one request. `flow import` and `flow verify` get the edges of every step node before they derive the fact layer.
- A daemon of the store keeps the servers warm. The first command starts it with the servers, which takes some seconds. The next commands take milliseconds for each node. The daemon stops after 10 minutes without a request. It keeps the search path of the command that started it: when a server program is not on that path, it answers the error and stops, so the next command starts a daemon with its own path. Its files are `daemon.lock`, `daemon.sock`, and `daemon.log` in the store; `index` and the daemon add them to the `.gitignore` of an older store.
- When `flow verify` cannot get the edges, for example in CI without a server, it prints a warning and verifies against the graph and its cache as they are.
- The edges go to `<graph>.semantic.json`, which records the byte digest of its graph. After a new index, the next command replaces the cache. The cache is not part of the flow binding, so it never makes a flow stale. `serve` sends each graph with the edges of its cache.
- Only a graph of the complete project folder, the folder that holds the store, gets semantic edges. For a graph of a subfolder, or of another root, these commands fail.
- `TEKDEK_NO_DAEMON=1` resolves in the command process: each command starts and stops its servers. `TEKDEK_DAEMON_IDLE_SECONDS=<n>` sets the idle time of the daemon. `TEKDEK_TIMINGS=1` prints the time of each stage to standard error.

Known limits:

- A Rust callee in a macro, or a function passed as a value, shows as a callee only after a `graph show` of that callee.
- The signature types and the value uses of a node show when the other end is resolved too. The edges between the steps of a flow are complete, because a flow resolves all its steps.
- rust-analyzer can miss some callers of trait methods.
