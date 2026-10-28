# Language Servers

`tekdek index` parses the source and needs no language server. `tekdek graph show`, `tekdek flow import`, and `tekdek flow verify` start the language server of each indexed language of the project, so check the servers before the first of them. The other commands need none.

| Language | Indexed | Server | Install | Size |
| --- | --- | --- | --- | --- |
| Rust | Yes | `rust-analyzer` | `rustup component add rust-analyzer`, `brew install rust-analyzer`, or a [release binary](https://github.com/rust-lang/rust-analyzer/releases) | ~40 MB |
| Python | Yes | `pyright-langserver` | `npm install -g pyright` | ~20 MB |
| Python, without Node.js | Yes | `pyright-langserver` | `uv tool install 'pyright[nodejs]'` or `pipx install 'pyright[nodejs]'` | ~240 MB |
| TypeScript, JavaScript | Yes | `tsc --lsp` of TypeScript 7 or later | `npm install -g typescript` | ~30 MB |
| Go | Not yet | `gopls` | `go install golang.org/x/tools/gopls@latest` | ~41 MB, plus the Go build caches |

Sources: [rust-analyzer](https://rust-analyzer.github.io), [pyright](https://github.com/microsoft/pyright), [TypeScript](https://github.com/microsoft/typescript-go), [gopls](https://go.dev/gopls/).

## Find a Local Server

Run `tekdek doctor <project root>`. It checks only the languages that have source files in the project. A `missing` server is not on `PATH`. A `broken` server is on `PATH` but cannot run, for example a rustup `rust-analyzer` proxy without the component, or a `tsc` older than TypeScript 7; treat it as missing. For a missing or broken server, look for a copy that is not on `PATH`, for example `rustup which rust-analyzer`, the project `.venv` or `node_modules/.bin`, or uv and pipx tools. Name a copy that you find in a configuration file, with its absolute path:

```toml
[languages.typescript]
program = "/path/to/project/node_modules/.bin/tsc"
```

Then pass `--config <file>` to `tekdek doctor`, `graph show`, `flow import`, and `flow verify`.

## Ask Before You Install

For each server that is still missing, ask the user in exactly this format, one server at a time, and wait for the answer:

```text
pyright needed for project acme-api - ~20 MB. y or n to download and install?
```

- The project is the name of the project root folder. The size is the size of the install that you will run.
- Install only after `y`. Treat any other answer as `n`.
- After the install, run `tekdek doctor <project root>` again. If the server is still not on `PATH`, name its full path in a configuration file as above.
- On `n`, offer to index only the other languages: write a configuration file with `[languages.<name>]` and `enabled = false`, and pass it with `--config <file>` to `tekdek index` and every later command. When no indexed language remains, stop.

## Language Not Indexed Yet

tekdek skips the files of a language that it does not index, so the flow cannot contain their functions. Tell the user once, and do not offer to install a server for that language. Name the server that support needs, so that the user can request it in [tekdek issue #4](https://github.com/dychen1/tekdek/issues/4): use the table, or do a web search for the standard server of another language and give its official page. When the project has no indexed language, stop.
