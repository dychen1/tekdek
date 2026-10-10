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
    process::{self, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::json;

use crate::analysis::language::ServerCommand;

/// One compiled fake server in its own folder.
///
/// Each test compiles its own server, because the program writes its
/// shutdown marker next to its executable.
pub(crate) struct FakeServer {
    directory: TestDirectory,
    executable: PathBuf,
}

impl FakeServer {
    pub(crate) fn new() -> Self {
        let directory = TestDirectory::new();
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

    pub(crate) fn shutdown_marker(&self) -> PathBuf {
        self.directory.path().join("shutdown-requested")
    }
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("tekdek-lsp-session-{}-{id}", process::id()));
        fs::create_dir(&path).expect("the fake server directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

const FAKE_SERVER: &str = include_str!("testdata/fake_lsp_server.rs");
