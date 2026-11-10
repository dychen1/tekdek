# CLI Reference

## Overview

- Purpose: list every `tekdek` command, its options, and its exit codes.
- Scope: the `index`, `graph`, `flow`, `diff`, `serve`, and `doctor` commands, and the configuration file.
- Boundaries: `tekdek help` prints the complete usage of the installed version. This page follows `main`.
- Outcome: a person or an agent can run each command without a look at the source.

## Commands

```text
tekdek index [<root>] [--name <name>] [--store <directory>] | [--output <path>]
  [--config <path>]

tekdek graph search <text> [--kind module|type|callable|field] [--limit <n>]
tekdek graph show <key>... [--config <path>]

tekdek flow list [--graph <file>]
tekdek flow show <name>
tekdek flow import <name> --graph <file> --input <path or -> [--check] [--config <path>]
  [--author agent|human|system] [--author-name <name>] [--model-id <id>]
tekdek flow verify <name> [--config <path>]
tekdek flow agent-contract

tekdek diff prepare [--base <commit or branch>]
tekdek diff import <name> --base <commit or branch> --before <flow> --after <flow>
  --input <path or -> [--check] [--author agent|human|system] [--author-name <name>] [--model-id <id>]
tekdek diff verify <name>
tekdek diff show <name>
tekdek diff list
tekdek diff agent-contract

tekdek serve [--store <directory>] [--port <n>] [--flow <name> | --diff <name>] [--open] [--no-index]
tekdek doctor [<root>] [--config <path>] [--json]
tekdek help
tekdek version
```

The `graph`, `flow`, and `diff` commands accept `--store <directory>` and `--json`. `--graph` takes a file name inside the store. Write `--option=value` for a value that starts with a hyphen.

## Exit Codes

Every command uses the same exit codes:

| Code | Meaning |
| --- | --- |
| 0 | The command succeeded. |
| 1 | A check found a problem: `flow verify` or `diff verify` found an issue, `flow import` got a `no_flow` answer, or `doctor` found a needed server that is missing or broken. |
| 2 | An error: an invalid command line, a missing store or graph, a rejected agent output, a failed analysis, or a failed language server. |

With `--json`, the result or the error is one JSON document on standard output. Without it, results go to standard output and errors go to standard error.

## Command Notes

| Command | Notes |
| --- | --- |
| `index` | The default root is the working directory, or the project root when a store exists. The default name is the root folder name. A root inside a project with a store needs `--name`. `--output` writes one graph and changes no store. |
| `graph search` | Lists the keys that contain every word, best name match first. Spaces, `::`, `:`, `.`, and `/` separate words, so `Command::parse` and `flow/import` work. The default limit is 20. JSON output names the graph file. |
| `graph show` | Shows the signature, documentation summary, container, members, callers, and callees of each key. A missing key names close keys. It reindexes the project graph if the source changed, then gets the edges of all the nodes in one request (see [Semantic Edges](analysis.md#semantic-edges)). |
| `flow list` | Lists the current generation of each flow, with its freshness. |
| `flow import` | `--author-name` alone records an agent author. Gets the edges of the step nodes first. |
| `flow verify` | Recomputes the derived layers and digests of the current generation. Gets the edges of the step nodes first, unless the flow is stale. |
| `diff import` | The Before flow comes from the base store, or else from this store. The After flow must be fresh. A function whose file has uncommitted changes is rejected. |
| `serve` | Serves on `127.0.0.1:8000` by default. Port 0 selects a free port. `--flow` prints the URL of one flow, `--diff` the URL of one diff, and `--open` opens it. |
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
