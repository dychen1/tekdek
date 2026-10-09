# Install tekdek

The installers put `tekdek` in `$XDG_BIN_HOME`, else `$XDG_DATA_HOME/../bin`, else `~/.local/bin`, and add that folder to `PATH`. Claude Code reads `PATH` when it starts, so a new install runs only by its full path until Claude Code restarts.

## Find an Installed Binary

When `tekdek version` reports that the command is not found, try these paths in order, and use the first that runs for every later command:

```sh
"$XDG_BIN_HOME/tekdek" version
"$XDG_DATA_HOME/../bin/tekdek" version
"$HOME/.local/bin/tekdek" version
```

## Install

Give the user the command for their system. Do not run it without the user's consent.

- macOS and Linux:

  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dychen1/tekdek/releases/latest/download/tekdek-installer.sh | sh
  ```

- Windows (PowerShell):

  ```powershell
  powershell -ExecutionPolicy Bypass -c "irm https://github.com/dychen1/tekdek/releases/latest/download/tekdek-installer.ps1 | iex"
  ```

To upgrade, run the same command again.

## While the Repository Is Private

The installers download from GitHub Releases without credentials, so they fail while the repository is private. A user with access can download the archive for their system with the GitHub CLI:

```sh
gh release download --repo dychen1/tekdek --pattern 'tekdek-aarch64-apple-darwin.tar.xz'
tar -xJf tekdek-aarch64-apple-darwin.tar.xz
mkdir -p ~/.local/bin && mv tekdek-aarch64-apple-darwin/tekdek ~/.local/bin/
```

The archives are `tekdek-aarch64-apple-darwin.tar.xz`, `tekdek-x86_64-apple-darwin.tar.xz`, `tekdek-x86_64-unknown-linux-gnu.tar.xz`, `tekdek-aarch64-unknown-linux-gnu.tar.xz`, and `tekdek-x86_64-pc-windows-msvc.zip`. The Linux binaries need glibc 2.34 or later.
