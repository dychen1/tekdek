//! Semantic daemon.
//!
//! This module owns the daemon of one store and its client. The daemon keeps
//! the language servers of the project warm between commands. A store, a graph
//! file, and node keys are the input of the client. The semantic cache of that
//! graph is the output, as `ensure_in_process` writes it.
//!
//! # Protocol
//!
//! The daemon holds the lock `<store>/daemon.lock`, which holds its process ID,
//! and listens on `<store>/daemon.sock` (mode 0600). A connection sends one
//! JSON line `{"graph": "<file>", "keys": [..], "config": "<file>",
//! "version": <n>}`, where `config` is optional and absolute, and `version` is
//! the cache version of the client. The daemon resolves the keys that the
//! cache lacks, writes the cache, and answers one JSON line:
//! `{"ok": true}` or `{"error": "..."}`. It answers one connection at a time
//! and writes its log to `<store>/daemon.log`.
//!
//! # Lifecycle
//!
//! A client that cannot connect starts `tekdek daemon --store <store>` in a new
//! process group and waits for the socket. When two daemons start, the one
//! that gets the lock listens and the other exits. A socket file without a lock
//! holder is stale, and the next daemon replaces it. When a daemon dies during
//! a request, the client starts a new daemon and sends the request one more
//! time. A daemon that does not start fails the request at once.
//!
//! The daemon parses the project at its first request, and the resolver starts
//! each server at the first key of its language. When the hash of the source
//! files changes, or the configuration names other language profiles, the
//! daemon stops the servers and parses again. A request that fails at its
//! server stops that server, and the next key of that language starts a new
//! one. After an idle time without a request, it removes its socket, frees its
//! lock, stops its servers, and exits.
//!
//! The daemon keeps the environment of the client that started it. So after
//! it answers that a server program is not on its search path, or that the
//! client has another cache version, it exits: the next client starts a daemon
//! with its own environment and program.
//!
//! The daemon owns the socket file that it bound, not the socket path. When
//! the path names another file or no file, for example after `rm -rf .tekdek`
//! or a move of the project folder, the daemon exits and leaves the path as it
//! is.
//!
//! # Sections
//!
//! `Client` sends one request and starts the daemon when necessary.
//! `Daemon` listens on the socket and resolves the requests.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    mem,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{SocketAddr, UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{self, Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{ensure_in_process, fill_cache, project_root, resolved};
use crate::{
    analysis::{
        PipelineConfig, PipelineError, language::LanguageProfile, resolve::Resolver, source_hash,
        timing,
    },
    store::{read_bytes, semantic::CACHE_VERSION, update_project_ignore},
};

/// The lock file of the daemon. It holds the process ID of the daemon.
const LOCK: &str = "daemon.lock";
/// The socket of the daemon.
const SOCKET: &str = "daemon.sock";
/// The log file of the daemon: its standard output and standard error.
const LOG: &str = "daemon.log";
/// The idle time after which a daemon exits. `TEKDEK_DAEMON_IDLE_SECONDS` replaces it.
const IDLE: Duration = Duration::from_mins(10);
/// The longest wait of a client for the socket of a daemon that it starts.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The interval at which a waiting client and an idle daemon check again.
const POLL: Duration = Duration::from_millis(10);
/// The longest wait of a daemon for the request line of a connection.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// A client empties a larger log when it starts a daemon.
const LOG_LIMIT: u64 = 1 << 20;
/// The number of log lines in the error of a failed daemon.
const LOG_TAIL: usize = 10;

/// One request line.
#[derive(Serialize, Deserialize)]
struct Request {
    graph: String,
    keys: Vec<String>,
    /// The absolute path of the configuration file, or none for the default file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config: Option<PathBuf>,
    /// The cache version of the client. A request without it has version 0.
    #[serde(default)]
    version: u32,
}

/// One answer line: `{"ok": true}` or `{"error": "..."}`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Response {
    Ok(bool),
    Error(String),
}

// -----------------------------------------------------------------------------
// Client
// -----------------------------------------------------------------------------

/// Resolves the keys that the cache of one graph lacks, with the daemon of the store.
///
/// The daemon program is this executable. With `TEKDEK_TIMINGS` set, the call
/// prints the time to connect, spawn, wait, and resolve to standard error.
pub(super) fn ensure(
    store: &Path,
    graph_file: &str,
    keys: &[String],
    config: Option<&Path>,
) -> io::Result<()> {
    let program = env::current_exe()?;
    ensure_with(store, graph_file, keys, config, |store| {
        let mut command = Command::new(&program);
        command.arg("daemon").arg("--store").arg(store);
        command
    })
}

/// Resolves the keys that the cache lacks, with the daemon that `daemon` starts for a canonical store.
///
/// A socket path that is too long for the system falls back to `ensure_in_process`.
fn ensure_with(
    store: &Path,
    graph_file: &str,
    keys: &[String],
    config: Option<&Path>,
    daemon: impl Fn(&Path) -> Command,
) -> io::Result<()> {
    let store = fs::canonicalize(store)?;
    if resolved(&store, graph_file, keys)? {
        return Ok(());
    }
    let socket = store.join(SOCKET);
    if SocketAddr::from_pathname(&socket).is_err() {
        return ensure_in_process(&store, graph_file, keys, config);
    }
    let line = serde_json::to_string(&Request {
        graph: graph_file.to_owned(),
        keys: keys.to_vec(),
        // The daemon runs in another folder.
        config: config.map(std::path::absolute).transpose()?,
        version: CACHE_VERSION,
    })?;
    let total = timing::start();
    let answer = connect(&store, &socket, &daemon, true)
        .and_then(|stream| match exchange(stream, &line) {
            // A daemon that dies during the request gets one successor.
            Err(_) => exchange(connect(&store, &socket, &daemon, false)?, &line),
            answer => answer,
        })
        .map_err(|error| with_log(&store, &error));
    timing::report(total);
    answer?
}

/// Connects to the daemon of a store, and starts one when none listens.
///
/// A started daemon that exits with success lost the lock to another daemon.
/// When the socket still refuses, the client starts one more daemon, until the
/// start timeout. Only the first start with `cut_log` empties a large log, so
/// a retry keeps the lines of a crash.
fn connect(
    store: &Path,
    socket: &Path,
    daemon: &impl Fn(&Path) -> Command,
    mut cut_log: bool,
) -> io::Result<UnixStream> {
    let started = timing::start();
    if let Ok(stream) = UnixStream::connect(socket) {
        timing::record_stage(started, || "daemon connect".to_owned());
        return Ok(stream);
    }
    let mut spawn = || -> io::Result<_> {
        let log = log_file(store, mem::take(&mut cut_log))?;
        daemon(store)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()
    };
    let mut child = spawn()?;
    timing::record_stage(started, || "daemon spawn".to_owned());
    let started = timing::start();
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            timing::record_stage(started, || "daemon wait".to_owned());
            reap(child);
            return Ok(stream);
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(io::Error::other(format!("the daemon exited with {status}")));
            }
            child = spawn()?;
        }
        if Instant::now() >= deadline {
            reap(child);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the daemon did not listen within {} s",
                    START_TIMEOUT.as_secs()
                ),
            ));
        }
        thread::sleep(POLL);
    }
}

/// Waits for a started daemon on its own thread, so a long-lived client leaves no zombie process.
fn reap(mut child: Child) {
    thread::spawn(move || child.wait());
}

/// Sends one request line and reads the answer.
///
/// The outer error is a failed connection, for example a daemon that died. The
/// inner error is the error that the daemon answers.
fn exchange(mut stream: UnixStream, line: &str) -> io::Result<io::Result<()>> {
    let started = timing::start();
    writeln!(stream, "{line}")?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    timing::record_stage(started, || "daemon resolve".to_owned());
    match serde_json::from_str(&answer) {
        Ok(Response::Ok(true)) => Ok(Ok(())),
        Ok(Response::Error(message)) => Ok(Err(io::Error::other(message))),
        _ => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the daemon closed the connection without an answer",
        )),
    }
}

/// Opens the log of the daemon for appending. With `cut`, it empties a log that is larger than the limit.
// shortcut: the limit applies when a daemon starts, so a daemon that runs for days can grow its log past it.
fn log_file(store: &Path, cut: bool) -> io::Result<File> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(store.join(LOG))?;
    if cut && log.metadata()?.len() > LOG_LIMIT {
        log.set_len(0)?;
    }
    Ok(log)
}

/// Adds the last lines of the daemon log to a failure of the daemon.
///
/// A log that is a symbolic link adds no lines, so a link committed into the store cannot show another file.
fn with_log(store: &Path, error: &io::Error) -> io::Error {
    let log = read_bytes(&store.join(LOG))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let lines = log.lines().collect::<Vec<_>>();
    io::Error::new(
        error.kind(),
        format!(
            "the semantic daemon failed: {error}\nlast lines of {}:\n{}",
            store.join(LOG).display(),
            lines[lines.len().saturating_sub(LOG_TAIL)..].join("\n")
        ),
    )
}

// -----------------------------------------------------------------------------
// Daemon
// -----------------------------------------------------------------------------

/// Serves the semantic requests of one store until the idle time passes without a request.
///
/// Returns at once when another daemon holds the lock of the store. Each
/// request writes one line with its times to standard error. `load` returns the
/// language profiles of the configuration file of a request.
///
/// # Errors
///
/// Returns an error if the lock, the socket, or the listener fails.
pub fn serve(
    store: &Path,
    load: impl Fn(Option<&Path>) -> io::Result<Vec<LanguageProfile>>,
) -> io::Result<()> {
    let store = fs::canonicalize(store)?;
    let root = project_root(&store)?.to_path_buf();
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(store.join(LOCK))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            log(&format!("another daemon serves {}", store.display()));
            return Ok(());
        }
        Err(fs::TryLockError::Error(error)) => return Err(error),
    }
    lock.set_len(0)?;
    write!(lock, "{}", process::id())?;
    if let Err(error) = update_project_ignore(&store) {
        log(&format!(
            "cannot add the daemon files to the ignore file: {error}"
        ));
    }

    let socket = store.join(SOCKET);
    let listener = bind(&store, &socket)?;
    let ours = identity(&socket)?;
    log(&format!(
        "daemon {} listens on {}",
        process::id(),
        socket.display()
    ));
    let mut daemon = Daemon {
        store,
        root,
        load,
        profiles: Vec::new(),
        resolver: None,
        exit: false,
    };
    let served = daemon.listen(&listener, &socket, ours, idle_time());
    // The socket goes before the lock, and only while it is ours, so this
    // daemon never removes the socket of the next one.
    let removed = if identity(&socket).ok() == Some(ours) {
        fs::remove_file(&socket)
    } else {
        Ok(())
    };
    drop(listener);
    drop(lock);
    daemon.stop();
    log(&format!("daemon {} exits", process::id()));
    served.and(removed)
}

/// Binds the socket of the store with mode 0600.
///
/// The bind happens in a private folder, and a rename moves the socket into
/// place, so its mode is never wider than 0600. The rename replaces a stale
/// socket: the lock holder owns the socket path.
fn bind(store: &Path, socket: &Path) -> io::Result<UnixListener> {
    // The short names keep the bound path shorter than the socket path.
    let private = store.join(format!("d{}", process::id()));
    // A dead daemon with the same process ID can leave its folder.
    let _ = fs::remove_dir_all(&private);
    fs::DirBuilder::new().mode(0o700).create(&private)?;
    let bound = (|| -> io::Result<_> {
        let path = private.join("s");
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        fs::rename(&path, socket)?;
        Ok(listener)
    })();
    let removed = fs::remove_dir_all(&private);
    let listener = bound?;
    removed?;
    Ok(listener)
}

/// Returns the device and inode numbers of a path, which name one file.
fn identity(path: &Path) -> io::Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

/// Returns the idle time: `TEKDEK_DAEMON_IDLE_SECONDS`, or ten minutes.
fn idle_time() -> Duration {
    env::var("TEKDEK_DAEMON_IDLE_SECONDS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(IDLE, Duration::from_secs)
}

/// Writes one log line with the current time.
fn log(message: &str) {
    eprintln!("{} {message}", jiff::Timestamp::now());
}

/// The state of one daemon: its store, its project, its profiles, and the warm resolver.
///
/// `exit` is true after an answer that another daemon must serve the next request.
struct Daemon<L> {
    store: PathBuf,
    root: PathBuf,
    load: L,
    profiles: Vec<LanguageProfile>,
    resolver: Option<Resolver>,
    exit: bool,
}

impl<L: Fn(Option<&Path>) -> io::Result<Vec<LanguageProfile>>> Daemon<L> {
    /// Answers connections until the idle time passes without one, the socket path stops naming the file `ours`, or an answer sets `exit`.
    ///
    /// A connection that arrives after the path changed gets no answer, so its
    /// client starts a new daemon.
    // shortcut: the listener polls every 10 ms for the idle time, upgrade to poll(2) if the latency matters.
    fn listen(
        &mut self,
        listener: &UnixListener,
        socket: &Path,
        ours: (u64, u64),
        idle: Duration,
    ) -> io::Result<()> {
        listener.set_nonblocking(true)?;
        let mut last = Instant::now();
        let owned = || {
            let owned = identity(socket).ok() == Some(ours);
            if !owned {
                log("the socket path names another file");
            }
            owned
        };
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if !owned() {
                        return Ok(());
                    }
                    if let Err(error) = self.answer(&stream) {
                        log(&format!("cannot answer a client: {error}"));
                    }
                    if self.exit {
                        return Ok(());
                    }
                    last = Instant::now();
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if last.elapsed() >= idle || !owned() {
                        return Ok(());
                    }
                    thread::sleep(POLL);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Reads one request from a connection and writes its answer.
    fn answer(&mut self, mut stream: &UnixStream) -> io::Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let resolved = serde_json::from_str::<Request>(&line)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid request: {error}"),
                )
            })
            .and_then(|request| self.resolve(&request));
        let response = match resolved {
            Ok(()) => Response::Ok(true),
            Err(error) => Response::Error(error.to_string()),
        };
        writeln!(stream, "{}", serde_json::to_string(&response)?)
    }

    /// Resolves the keys of one request, and restarts the servers first when the profiles or the source files changed.
    fn resolve(&mut self, request: &Request) -> io::Result<()> {
        if request.version != CACHE_VERSION {
            self.exit = true;
            log("a client has another cache version: exit");
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the daemon writes cache version {CACHE_VERSION}, not {}: it exits, so try again",
                    request.version
                ),
            ));
        }
        let started = Instant::now();
        let profiles = (self.load)(request.config.as_deref())?;
        if profiles != self.profiles {
            if self.resolver.is_some() {
                log("the language profiles changed: restart the language servers");
            }
            self.stop();
            self.profiles = profiles;
        }
        let mut config = PipelineConfig::new(self.root.clone());
        config.profiles.clone_from(&self.profiles);
        let current = source_hash(&config).map_err(io::Error::other)?;
        let hashed = started.elapsed();
        if self
            .resolver
            .as_ref()
            .is_some_and(|resolver| resolver.source_hash() != current)
        {
            log("the source files changed: restart the language servers");
            self.stop();
        }
        let resolver = match self.resolver.take() {
            Some(resolver) => resolver,
            None => Resolver::new(&self.root, self.profiles.clone()).map_err(io::Error::other)?,
        };
        let parsed = started.elapsed().saturating_sub(hashed);
        let resolver = self.resolver.insert(resolver);
        let filled = fill_cache(resolver, &self.store, &request.graph, &request.keys);
        let answered = started.elapsed().saturating_sub(hashed + parsed);
        let outcome = match &filled {
            Ok(edges) => format!("{} edges", edges.len()),
            Err(error) => format!("error: {error}"),
        };
        log(&format!(
            "{} {} keys: hash {:.3}s, parse {:.3}s, resolve {:.3}s, {outcome}",
            request.graph,
            request.keys.len(),
            hashed.as_secs_f64(),
            parsed.as_secs_f64(),
            answered.as_secs_f64(),
        ));
        if filled.as_ref().is_err_and(missing_server) {
            self.exit = true;
            log("a server program is not on the search path of this daemon: exit");
        }
        filled.map(drop)
    }

    /// Stops the language servers, if they run.
    fn stop(&mut self) {
        if let Some(resolver) = self.resolver.take()
            && let Err(error) = resolver.shutdown()
        {
            log(&format!("cannot stop the language servers: {error}"));
        }
    }
}

/// Returns true for the error of a server program that is not on the search path.
fn missing_server(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<PipelineError>())
        .is_some_and(|inner| matches!(inner, PipelineError::MissingServer(_)))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(all(test, feature = "python"))]
mod tests {
    use std::{
        env, fs,
        io::{BufRead, BufReader, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        process::Command,
        sync::Barrier,
        thread,
        time::{Duration, Instant},
    };

    use serde_json::json;

    use super::{LOCK, LOG, SOCKET, ensure_with, serve};
    use crate::{
        analysis::{language::ServerCommand, lsp::fake_server::FakeServer},
        semantic::tests::{HELPER, RUN, index, project, scripted, with_server},
        store::{digest_of, graph::RelationKind, semantic::read_cache},
    };

    /// The store of the daemon that a test starts.
    const TEST_STORE: &str = "TEKDEK_TEST_DAEMON_STORE";
    /// The fake server command of the daemon that a test starts.
    const TEST_SERVER: &str = "TEKDEK_TEST_DAEMON_SERVER";

    /// Serves as the daemon when a test starts this test binary as one, and does nothing otherwise.
    #[test]
    fn serves_as_the_daemon_that_a_test_starts() {
        let (Some(store), Ok(server)) = (env::var_os(TEST_STORE), env::var(TEST_SERVER)) else {
            return;
        };
        // A configuration file of a test holds the command of another server.
        let load = |config: Option<&Path>| {
            let server = config.map_or_else(|| Ok(server.clone()), fs::read_to_string)?;
            let (program, arguments, initialization_options) =
                serde_json::from_str(&server).expect("the server command should decode");
            Ok(with_server(&ServerCommand {
                program,
                arguments,
                initialization_options,
            }))
        };

        serve(Path::new(&store), load).expect("the daemon should serve");
    }

    #[test]
    fn restarts_the_servers_when_the_configuration_changes() {
        let project = project();
        let store = store(&project);
        let (first, second) = (FakeServer::new(), FakeServer::new());
        let daemon = daemon(&first, "10", "0");
        ensure_with(&store, "graph.json", &[RUN.to_owned()], None, &daemon).expect("first");
        let config = project.path().join("config.json");
        fs::write(&config, server_json(&second)).expect("configuration");

        ensure_with(
            &store,
            "graph.json",
            &[HELPER.to_owned()],
            Some(&config),
            &daemon,
        )
        .expect("second");

        assert!(first.shutdown_marker().exists(), "the first server stops");
        assert!(!second.requests().is_empty(), "the second server answers");
        kill(&store, "-TERM");
    }

    #[test]
    fn starts_a_daemon_reuses_it_and_keeps_its_server_after_a_key_error() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        let daemon = daemon(&server, "10", "0");

        ensure_with(&store, "graph.json", &[RUN.to_owned()], None, &daemon).expect("first client");
        let first = pid(&store);
        let error = ensure_with(
            &store,
            "graph.json",
            &["lib.py::callable:none".to_owned()],
            None,
            &daemon,
        )
        .expect_err("the key names no node");
        ensure_with(&store, "graph.json", &[HELPER.to_owned()], None, &daemon)
            .expect("third client");

        assert!(
            error.to_string().contains("no node with the key"),
            "{error}"
        );
        assert_eq!(pid(&store), first);
        assert_eq!(starts(&server), 1, "the server stays warm");
        let mode = fs::metadata(store.join(SOCKET))
            .expect("socket")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(
            !store.join(format!("d{first}")).exists(),
            "the bind folder goes"
        );
        let cache = cache(&store);
        assert_eq!(cache.resolved.iter().collect::<Vec<_>>(), [HELPER, RUN]);
        assert_eq!(
            cache
                .edges
                .iter()
                .map(|edge| (edge.source.as_str(), edge.target.as_str(), edge.kind))
                .collect::<Vec<_>>(),
            [(RUN, HELPER, RelationKind::Calls)]
        );
        kill(&store, "-TERM");
    }

    #[test]
    fn fails_at_once_when_the_daemon_does_not_start() {
        let project = project();
        let store = store(&project);
        let starts = project.path().join("starts.txt");
        let daemon = |_: &Path| {
            let mut command = Command::new("sh");
            command
                .args(["-c", "echo start >> \"$0\"; exit 3"])
                .arg(&starts);
            command
        };

        let error = ensure_with(&store, "graph.json", &[RUN.to_owned()], None, daemon)
            .expect_err("the daemon exits with an error");

        assert!(error.to_string().contains("exited with"), "{error}");
        assert_eq!(fs::read_to_string(&starts).expect("starts"), "start\n");
    }

    #[test]
    fn exits_after_a_missing_server_program_so_a_new_daemon_serves_the_next_client() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        let daemon = daemon(&server, "10", "0");
        let config = project.path().join("config.json");
        fs::write(&config, json!(["/missing/server", [], {}]).to_string()).expect("configuration");
        let error = ensure_with(
            &store,
            "graph.json",
            &[RUN.to_owned()],
            Some(&config),
            &daemon,
        )
        .expect_err("the server program is missing");
        assert!(
            error.to_string().contains("needs /missing/server"),
            "{error}"
        );
        let first = pid(&store);
        wait_until("the exit of the daemon", || {
            fs::read_to_string(store.join(LOG))
                .unwrap_or_default()
                .contains(&format!("daemon {first} exits"))
        });

        // The new daemon reads the corrected configuration, as it reads a new search path.
        fs::write(&config, server_json(&server)).expect("corrected configuration");
        ensure_with(
            &store,
            "graph.json",
            &[RUN.to_owned()],
            Some(&config),
            &daemon,
        )
        .expect("the corrected server answers");

        assert_ne!(pid(&store), first);
        assert!(cache(&store).resolved.contains(RUN));
        kill(&store, "-TERM");
    }

    #[test]
    fn exits_after_a_request_of_another_cache_version() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        ensure_with(
            &store,
            "graph.json",
            &[RUN.to_owned()],
            None,
            daemon(&server, "10", "0"),
        )
        .expect("the client starts a daemon");
        let first = pid(&store);
        let mut stream = UnixStream::connect(store.join(SOCKET)).expect("the daemon listens");

        writeln!(stream, r#"{{"graph":"graph.json","keys":[],"version":0}}"#).expect("request");
        let mut answer = String::new();
        BufReader::new(&stream)
            .read_line(&mut answer)
            .expect("answer");

        assert!(answer.contains("cache version"), "{answer}");
        wait_until("the exit of the daemon", || {
            !store.join(SOCKET).exists()
                && fs::read_to_string(store.join(LOG))
                    .unwrap_or_default()
                    .contains(&format!("daemon {first} exits"))
        });
    }

    #[test]
    fn leaves_the_socket_of_a_new_daemon_when_the_store_files_are_removed() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        let daemon = daemon(&server, "10", "0");
        ensure_with(&store, "graph.json", &[RUN.to_owned()], None, &daemon).expect("first daemon");
        let old = pid(&store);
        fs::remove_file(store.join(SOCKET)).expect("remove the socket");
        fs::remove_file(store.join(LOCK)).expect("remove the lock");

        ensure_with(&store, "graph.json", &[HELPER.to_owned()], None, &daemon)
            .expect("a new daemon answers");

        assert_ne!(pid(&store), old);
        wait_until("the exit of the old daemon", || {
            fs::read_to_string(store.join(LOG))
                .unwrap_or_default()
                .contains(&format!("daemon {old} exits"))
        });
        assert!(
            UnixStream::connect(store.join(SOCKET)).is_ok(),
            "the socket of the new daemon stays"
        );
        kill(&store, "-TERM");
    }

    #[test]
    fn replaces_a_stale_socket_file_and_ignores_the_daemon_files_of_an_older_store() {
        let project = project();
        let store = store(&project);
        drop(UnixListener::bind(store.join(SOCKET)).expect("a socket without a daemon"));
        fs::write(store.join(".gitignore"), ".store.lock\n").expect("older ignore file");
        let server = FakeServer::new();

        ensure_with(
            &store,
            "graph.json",
            &[RUN.to_owned()],
            None,
            daemon(&server, "10", "0"),
        )
        .expect("the client starts a daemon");

        assert!(cache(&store).resolved.contains(RUN));
        assert_eq!(
            fs::read_to_string(store.join(".gitignore")).expect("ignore file"),
            ".store.lock\ndaemon.lock\ndaemon.log\ndaemon.sock\n"
        );
        kill(&store, "-TERM");
    }

    #[test]
    fn starts_a_new_daemon_once_when_the_daemon_dies_during_a_request() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        let daemon = daemon(&server, "10", "300");

        thread::scope(|scope| {
            let client =
                scope.spawn(|| ensure_with(&store, "graph.json", &[RUN.to_owned()], None, &daemon));
            wait_until("the first server request", || !server.requests().is_empty());
            let first = pid(&store);
            kill(&store, "-KILL");

            client
                .join()
                .expect("client thread")
                .expect("the second daemon answers");
            assert_ne!(pid(&store), first);
        });
        assert!(cache(&store).resolved.contains(RUN));
        kill(&store, "-TERM");
    }

    #[test]
    fn exits_after_the_idle_time_and_removes_its_socket() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();

        ensure_with(
            &store,
            "graph.json",
            &[RUN.to_owned()],
            None,
            daemon(&server, "1", "0"),
        )
        .expect("the client starts a daemon");

        wait_until("the idle exit", || {
            !store.join(SOCKET).exists() && server.shutdown_marker().exists()
        });
        let lock = fs::File::open(store.join(LOCK)).expect("lock file");
        lock.try_lock().expect("the daemon frees its lock");
    }

    #[test]
    fn restarts_the_servers_when_the_source_files_change() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        let daemon = daemon(&server, "10", "0");
        ensure_with(&store, "graph.json", &[RUN.to_owned()], None, &daemon).expect("first request");
        let first = pid(&store);

        let source = project.path().join("lib.py");
        let text = fs::read_to_string(&source).expect("source");
        fs::write(&source, text + "def extra():\n    pass\n").expect("changed source");
        index(project.path());
        ensure_with(&store, "graph.json", &[RUN.to_owned()], None, &daemon)
            .expect("second request");

        assert_eq!(pid(&store), first);
        assert!(server.shutdown_marker().exists(), "the old server stops");
        assert!(cache(&store).resolved.contains(RUN));
        kill(&store, "-TERM");
    }

    #[test]
    fn starts_one_daemon_for_two_clients_at_once() {
        let project = project();
        let store = store(&project);
        let server = FakeServer::new();
        let daemon = daemon(&server, "10", "0");
        let barrier = Barrier::new(2);

        thread::scope(|scope| {
            let clients = [RUN, HELPER].map(|key| {
                let (store, daemon, barrier) = (&store, &daemon, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    ensure_with(store, "graph.json", &[key.to_owned()], None, daemon)
                })
            });
            for client in clients {
                client
                    .join()
                    .expect("client thread")
                    .expect("the client gets an answer");
            }
        });

        let log = fs::read_to_string(store.join(LOG)).expect("log");
        assert_eq!(log.matches(" listens on ").count(), 1, "{log}");
        assert_eq!(cache(&store).resolved.len(), 2);
        kill(&store, "-TERM");
    }

    /// Returns the canonical store of a test project.
    fn store(project: &tempfile::TempDir) -> PathBuf {
        fs::canonicalize(project.path().join(".tekdek")).expect("store")
    }

    /// Returns a command builder that starts this test binary as the daemon of a store.
    fn daemon<'a>(
        server: &FakeServer,
        idle: &'a str,
        delay: &'a str,
    ) -> impl Fn(&Path) -> Command + 'a {
        let server = server_json(server);
        move |store| {
            let mut command = Command::new(env::current_exe().expect("the test binary"));
            command
                .args([
                    "--exact",
                    "semantic::daemon::tests::serves_as_the_daemon_that_a_test_starts",
                    "--nocapture",
                ])
                .env(TEST_STORE, store)
                .env(TEST_SERVER, &server)
                .env("TEKDEK_DAEMON_IDLE_SECONDS", idle)
                .env("FAKE_LSP_DELAY_MS", delay);
            command
        }
    }

    /// Returns the scripted command of a fake server as the JSON that the test daemon reads.
    fn server_json(server: &FakeServer) -> String {
        let server = scripted(server);
        json!([
            server.program,
            server.arguments,
            server.initialization_options
        ])
        .to_string()
    }

    /// Returns the number of times that a fake server started.
    fn starts(server: &FakeServer) -> usize {
        server
            .requests()
            .iter()
            .filter(|method| *method == "initialize")
            .count()
    }

    /// Returns the process ID in the daemon lock.
    fn pid(store: &Path) -> String {
        fs::read_to_string(store.join(LOCK)).expect("the daemon lock")
    }

    /// Sends a signal to the daemon of a store.
    fn kill(store: &Path, signal: &str) {
        let status = Command::new("kill")
            .args([signal, &pid(store)])
            .status()
            .expect("kill should start");
        assert!(status.success());
    }

    /// Returns the cache of the current `graph.json`.
    fn cache(store: &Path) -> crate::store::semantic::SemanticCache {
        let digest = digest_of(&fs::read(store.join("graph.json")).expect("graph bytes"));
        read_cache(store, "graph.json", &digest).expect("cache")
    }

    /// Waits up to 20 s for a condition.
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shows_no_lines_of_a_log_that_links_to_another_file() {
        let directory = tempfile::tempdir().expect("temporary folder");
        let secret = directory.path().join("secret");
        fs::write(&secret, "private key\n").expect("secret file");
        std::os::unix::fs::symlink(&secret, directory.path().join(LOG)).expect("log link");

        let error = super::with_log(directory.path(), &std::io::Error::other("stopped"));

        assert!(!error.to_string().contains("private key"), "{error}");
    }
}
