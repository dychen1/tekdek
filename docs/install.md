# Install

## Overview

- Purpose: install tekdek and the language servers that it uses.
- Scope: the plugin and its launcher, the CLI installers, the build from source, and the language servers.
- Boundaries: `tekdek index` and `tekdek serve` need no language server. Go is not indexed yet.
- Outcome: a `tekdek` binary that an agent or a person can run, and a server for each indexed language of the project.

## The Plugin

Install the plugin in your coding agent. It adds the `flow`, `flow-view`, and `diff` skills.

- Claude Code:

  ```text
  /plugin marketplace add dychen1/tekdek
  /plugin install tekdek@tekdek
  ```

- Codex:

  ```sh
  codex plugin marketplace add dychen1/tekdek
  codex plugin add tekdek@tekdek
  ```

- The marketplace pins the plugin to the latest release tag, so the skills always match a released binary. Both agents read the same `.claude-plugin/marketplace.json`.
- The launcher `plugin/bin/tekdek` downloads the binary of the plugin version on the first run. It caches the binary in `~/.cache/tekdek/<version>/` and checks its sha256. When the direct download fails, it tries the GitHub CLI.
- Claude Code puts the launcher on `PATH` after your own `PATH`, so an installed `tekdek` runs first. Codex runs it by its path.
- Claude Code does not update plugins from third-party marketplaces by default. Turn on auto-update for the `tekdek` marketplace in `/plugin`, or run `/plugin marketplace update tekdek`. In Codex, run `codex plugin marketplace upgrade tekdek`.

## The CLI Alone

To run `tekdek` without an agent, or before Claude Code restarts:

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

The installers need no Rust or Node.js. They put `tekdek` in `$XDG_BIN_HOME`, else `$XDG_DATA_HOME/../bin`, else `~/.local/bin`, and add that folder to `PATH`. Run the same command to upgrade.

The Linux binaries need glibc 2.34 or later: Ubuntu 22.04, Debian 12, RHEL 9, Amazon Linux 2023, or newer. Alpine and older distributions cannot run them; build from source there.

## Language Servers

`tekdek index` parses the source and starts no language server. `tekdek graph show`, `tekdek flow import`, and `tekdek flow verify` get calls, references, and types from a language server, only for the nodes that they read (see [Semantic Edges](analysis.md#semantic-edges)). The other commands need no language server.

| Language | Indexed | Server | Install |
| --- | --- | --- | --- |
| Rust | Yes | `rust-analyzer` | `rustup component add rust-analyzer`, `brew install rust-analyzer`, or a [release binary](https://github.com/rust-lang/rust-analyzer/releases) |
| Python | Yes | `pyright-langserver` | `npm install -g pyright`, or without Node.js: `uv tool install 'pyright[nodejs]'` or `pipx install 'pyright[nodejs]'` |
| TypeScript and JavaScript | Yes | `tsc --lsp` of TypeScript 7 | `npm install -g typescript` |
| Go | Not yet | `gopls` | `go install golang.org/x/tools/gopls@latest` |

- `tekdek doctor` prints the path and version of each server, or the install command of a missing server. It runs the version command of each server, so a server that cannot run, such as a rustup proxy without the component, is `broken`. pyright reports its version through the `pyright` program next to `pyright-langserver`; without it, the version is unverified.
- `tekdek doctor <root>` checks only the languages that have source files in the root. A Rust project then does not need pyright.
- Name a server that is not on `PATH` in the [configuration file](cli.md#configuration), for example `program = ".venv/bin/pyright-langserver"` in `[languages.python]`.
- The TypeScript server is the `tsc` program of TypeScript 7 or later. Earlier versions have no `--lsp` option. To use the TypeScript of a project, set `program` in `[languages.typescript]` to the absolute path of its `node_modules/.bin/tsc`.
- The TypeScript server resolves ES module imports in TypeScript and JavaScript. It does not resolve `require` calls, so a CommonJS call into another file gets no edge.
- `tekdek index` skips the files of a language that it does not index yet. Request a language in [issue #4](https://github.com/dychen1/tekdek/issues/4).
- `/tekdek:flow` looks for a local server first and asks before it installs one, for example `pyright needed for project acme-api - ~20 MB. y or n to download and install?`.
