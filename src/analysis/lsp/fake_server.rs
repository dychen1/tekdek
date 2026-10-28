//! Fake language server for tests.
//!
//! This module compiles the standalone program in
//! `testdata/fake_lsp_server.rs` into one test folder and builds server
//! commands that start it. The command argument selects the server mode.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::json;
use tempfile::TempDir;

use crate::analysis::language::ServerCommand;

/// One compiled fake server in its own folder.
///
/// Each test compiles its own server, because the program writes its
/// shutdown marker next to its executable.
pub(crate) struct FakeServer {
    directory: TempDir,
    executable: PathBuf,
}

impl FakeServer {
    pub(crate) fn new() -> Self {
        let directory = tempfile::tempdir().expect("the fake server folder should be created");
        let source = directory.path().join("fake_server.rs");
        let executable = directory
            .path()
            .join(format!("fake_server{}", env::consts::EXE_SUFFIX));
        fs::write(&source, FAKE_SERVER).expect("the fake server source should be written");
        let compiler = env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
        let output = Command::new(compiler)
            .arg("--edition=2024")
            .arg("-Dwarnings")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("rustc should start for the fake server");
        assert!(
            output.status.success(),
            "the fake server should compile: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        Self {
            directory,
            executable,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn command(&self, mode: &str) -> ServerCommand {
        ServerCommand {
            program: self.executable.to_string_lossy().into_owned(),
            arguments: vec![mode.to_owned()],
            initialization_options: json!({"analysis": {"mode": "test"}}),
        }
    }

    /// Returns a command that answers requests from a script, as the program documentation states.
    #[cfg_attr(
        not(feature = "python"),
        allow(dead_code, reason = "only the Python resolver tests script a server")
    )]
    pub(crate) fn scripted(&self, script: &str) -> ServerCommand {
        let path = self.directory.path().join("script.txt");
        fs::write(&path, script).expect("the fake server script should be written");
        let mut command = self.command("script");
        command.arguments.push(path.to_string_lossy().into_owned());
        command
    }

    /// Returns the method of each request that a scripted server answered, in order.
    #[cfg_attr(
        not(feature = "python"),
        allow(dead_code, reason = "only the Python resolver tests script a server")
    )]
    pub(crate) fn requests(&self) -> Vec<String> {
        fs::read_to_string(self.directory.path().join("requests.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub(crate) fn shutdown_marker(&self) -> PathBuf {
        self.directory.path().join("shutdown-requested")
    }
}

const FAKE_SERVER: &str = include_str!("testdata/fake_lsp_server.rs");
