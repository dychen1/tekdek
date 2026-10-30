# Install tekdek

## Use the Plugin Launcher

The plugin ships the launcher `bin/tekdek`, two folders above this file. Claude Code puts it on `PATH` after the user's own `PATH`, so an installed `tekdek` runs first. Other agents, such as Codex, run it by its full path, for example `<plugin root>/bin/tekdek version`, and use that path for every later command.

On the first run, the launcher downloads the release binary of the plugin version into `~/.cache/tekdek/<version>/` and checks its sha256. It needs network access, and for the private repository an authenticated GitHub CLI (`gh auth login`). When the download fails, give the user the install command below and stop.

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

To upgrade, run the same command again. The installers download without credentials, so they fail while the repository is private; the launcher works then.
