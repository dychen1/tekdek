# Language Servers

## Overview

- Purpose: get the language servers that `tekdek index` needs, with the consent of the user.
- Scope: language detection, a search for local servers, one consent prompt for each missing server, the install, and languages that tekdek does not index yet.
- When: before `tekdek index`. Skip this procedure when the store already has a current graph of the project. The `graph`, `flow`, and `serve` commands need no language server.
- Rule: install a server only after the user answers `y`.
- Outcome: `tekdek doctor` reports `ok` for each language that the project needs, or the user knows which language tekdek skips and why.

## Support

| Language | Extensions | Indexed by tekdek | Server program |
| --- | --- | --- | --- |
| Rust | `.rs` | Yes | `rust-analyzer` |
| Python | `.py`, `.pyi` | Yes | `pyright-langserver` (from pyright) |
| TypeScript and JavaScript | `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` | Not yet | `typescript-language-server` |
| Go | `.go` | Not yet | `gopls` |
| Other languages | | Not yet | Find it with a web search (see [Language not yet indexed](#language-not-yet-indexed)) |

## Install Sources

Use the first row of a server whose prerequisite is on the machine. The size is the disk space that the install adds. Show the size of the row that you will run.

| Server | Prerequisite | Install command | Size, about | Official source |
| --- | --- | --- | --- | --- |
| rust-analyzer | rustup | `rustup component add rust-analyzer` | 40 MB | <https://rust-analyzer.github.io/book/rust_analyzer_binary.html> |
| rust-analyzer | Homebrew | `brew install rust-analyzer` | 40 MB | <https://formulae.brew.sh/formula/rust-analyzer> |
| rust-analyzer | None | Download `rust-analyzer-<target>.gz` from the latest release, unpack it to `~/.local/bin/rust-analyzer`, and make it executable. | 40 MB (15 MB download) | <https://github.com/rust-lang/rust-analyzer/releases> |
| pyright | Node.js | `npm install -g pyright` | 20 MB | <https://www.npmjs.com/package/pyright> |
| pyright | uv | `uv tool install 'pyright[nodejs]'` | 240 MB, with its own Node.js | <https://pypi.org/project/pyright/> |
| pyright | pipx | `pipx install 'pyright[nodejs]'` | 240 MB, with its own Node.js | <https://pypi.org/project/pyright/> |
| typescript-language-server | Node.js 22.22 or later | `npm install -g typescript-language-server typescript@6` | 27 MB | <https://github.com/typescript-language-server/typescript-language-server> |
| gopls | Go | `go install golang.org/x/tools/gopls@latest` | 41 MB, and the Go caches grow by up to 600 MB during the build | <https://go.dev/gopls/> |

The pyright project is <https://github.com/microsoft/pyright>. The PyPI package is a community wrapper that runs the same npm package.

## Procedure

1. Find the languages of the project. In a git repository, count the extensions of `git ls-files`. Map each extension to the Support table.
2. For each language that tekdek does not index yet, follow [Language not yet indexed](#language-not-yet-indexed).
3. Run `tekdek doctor --json`. For each indexed language that the project has:
   - `ok`: the server is on `PATH`. Use it.
   - `disabled`: the user configuration turns the language off. Do not change it.
   - `missing`: look for a local copy before you ask (step 4).
4. Look for a local copy that is not on `PATH`:
   - rust-analyzer: `rustup which rust-analyzer`, `~/.local/bin/rust-analyzer`, and the server folder of an editor extension, for example `~/.vscode/extensions/rust-lang.rust-analyzer-*/server/rust-analyzer`.
   - pyright: `.venv/bin/pyright-langserver` and `node_modules/.bin/pyright-langserver` in the project, `~/.local/bin/pyright-langserver`, `uv tool list`, `pipx list`, and `npm ls -g pyright`.

   When you find a copy, tell the user its path and pass it to the index: `tekdek index --server <language>=<path>`. Do not install a second copy.
5. rustup installs a `rust-analyzer` proxy in `~/.cargo/bin` also when the component is not installed. `tekdek doctor` then reports `ok` without a version. If `rust-analyzer --version` fails, the server is missing.
6. Ask the user about each missing server, one server at a time, in exactly this format, and wait for the answer:

   ```text
   pyright needed for project acme-api - 20 MB. y or n to download and install?
   ```

   The project name is the name of the project root folder. The size comes from the install row that you will run. Only `y` is consent. Treat any other answer as `n`.
7. On `y`, run the install command. Then run `tekdek doctor` again. If the server is still missing, its install folder is not on `PATH`: `$(npm prefix -g)/bin` for npm, `~/.local/bin` for uv and pipx, and `$(go env GOPATH)/bin` for Go. Pass the full path with `--server <language>=<path>`, and tell the user which folder to add to `PATH`.
8. On `n`, tekdek cannot index that language. When the project has another indexed language, offer to index only the other languages. Write a configuration file outside the project, for example `/tmp/tekdek-config.toml`:

   ```toml
   [languages.python]
   enabled = false
   ```

   Then run `tekdek index --config /tmp/tekdek-config.toml`. The flow then has no functions of the skipped language. When no indexed language remains, stop and tell the user that the flow needs the server.

## Language Not Yet Indexed

tekdek skips the files of a language without an adapter. The graph and the flow cannot contain their functions.

- Tell the user once, in one line, for example: `tekdek does not index TypeScript yet; the flow covers only the Python code.`
- Do not offer to install a server for such a language. tekdek does not use it yet.
- When the project has no indexed language, stop. `tekdek index` fails with `the codebase has no enabled supported source files`. Do not write a flow from source reading alone.
- Name the server that support needs, so that the user can request it in tekdek issue #4 (<https://github.com/dychen1/tekdek/issues/4>):
  - For TypeScript, JavaScript, and Go, use the Install Sources table.
  - For another language, do a web search for its standard language server. Propose the server that the language project maintains or recommends, with its official download page. Say that it is a proposal, in this format: `<language> is not indexed yet. Its standard language server is <server> (<official page>). Add it to tekdek issue #4 to request support.`
