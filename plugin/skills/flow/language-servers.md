# Language Servers

`tekdek index` needs a language server for each indexed language of the project. The `graph`, `flow`, and `serve` commands need none.

| Language | Indexed | Server | Install | Size |
| --- | --- | --- | --- | --- |
| Rust | Yes | `rust-analyzer` | `rustup component add rust-analyzer`, `brew install rust-analyzer`, or a [release binary](https://github.com/rust-lang/rust-analyzer/releases) | ~40 MB |
| Python | Yes | `pyright-langserver` | `npm install -g pyright` | ~20 MB |
| Python, without Node.js | Yes | `pyright-langserver` | `uv tool install 'pyright[nodejs]'` or `pipx install 'pyright[nodejs]'` | ~240 MB |
| TypeScript, JavaScript | Not yet | `typescript-language-server` | `npm install -g typescript-language-server typescript@6` | ~27 MB |
| Go | Not yet | `gopls` | `go install golang.org/x/tools/gopls@latest` | ~41 MB, plus the Go build caches |

Sources: [rust-analyzer](https://rust-analyzer.github.io), [pyright](https://github.com/microsoft/pyright), [typescript-language-server](https://github.com/typescript-language-server/typescript-language-server), [gopls](https://go.dev/gopls/).

## Find a Local Server

Run `tekdek doctor <project root>`. It checks only the languages that have source files in the project. A `missing` server is not on `PATH`. A `broken` server is on `PATH` but cannot run, for example a rustup `rust-analyzer` proxy without the component; treat it as missing. For a missing or broken server, look for a copy that is not on `PATH`, for example `rustup which rust-analyzer`, the project `.venv` or `node_modules/.bin`, or uv and pipx tools. Pass a copy that you find with `tekdek index --server <language>=<path>`.

## Ask Before You Install

For each server that is still missing, ask the user in exactly this format, one server at a time, and wait for the answer:

```text
pyright needed for project acme-api - ~20 MB. y or n to download and install?
```

- The project is the name of the project root folder. The size is the size of the install that you will run.
- Install only after `y`. Treat any other answer as `n`.
- After the install, run `tekdek doctor <project root>` again. If the server is still not on `PATH`, pass its full path with `--server`.
- On `n`, offer to index only the other languages: pass a configuration file with `[languages.<name>]` and `enabled = false` to `tekdek index --config <file>`. When no indexed language remains, stop.

## Language Not Indexed Yet

tekdek skips the files of a language that it does not index, so the flow cannot contain their functions. Tell the user once, and do not offer to install a server for that language. Name the server that support needs, so that the user can request it in [tekdek issue #4](https://github.com/dychen1/tekdek/issues/4): use the table, or do a web search for the standard server of another language and give its official page. When the project has no indexed language, stop.
