//! Shared language-server session.
//!
//! This module owns server initialization, capability validation, readiness
//! waiting, document capture, capture validation, normalization, and
//! shutdown. A session configuration and selected sources are the inputs.
//! Semantic facts are the output.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use lsp_types::{
    CallHierarchyServerCapability, ClientCapabilities, DidOpenTextDocumentParams,
    DocumentSymbolClientCapabilities, DocumentSymbolParams, DynamicRegistrationClientCapabilities,
    GotoCapability, InitializeParams, OneOf, PartialResultParams, ServerCapabilities,
    SymbolKindCapability, TextDocumentClientCapabilities, TextDocumentIdentifier, TextDocumentItem,
    TypeDefinitionProviderCapability, Uri, WorkDoneProgressParams, WorkspaceFolder,
    notification::DidOpenTextDocument, request::DocumentSymbolRequest,
};
use serde_json::{Value, json};

use crate::analysis::{
    fact::SemanticFact,
    language::{CapabilityPolicy, Language, RequiredCapability, ServerCommand},
    source::Sources,
};

use super::{
    CapturedDocument, FallbackPolicy, LspClient, LspError, SemanticCapture, capture, file_uri,
};

pub struct SessionConfig<'a> {
    pub root: &'a Path,
    pub language: Language,
    pub language_id: &'a str,
    pub server: &'a ServerCommand,
    pub required_capabilities: CapabilityPolicy,
    pub fallback: FallbackPolicy,
    pub readiness: ServerReadiness,
}

/// States when a server answers semantic requests correctly.
#[derive(Clone, Copy)]
pub enum ServerReadiness {
    /// The server answers every request correctly at once.
    #[cfg_attr(
        not(feature = "python"),
        allow(dead_code, reason = "only the Python adapter uses immediate readiness")
    )]
    Immediate,
    /// The server reports readiness through `experimental/serverStatus`.
    ///
    /// Such a server answers semantic requests with empty results while it
    /// loads the workspace. The session waits for a quiescent status before
    /// document capture.
    #[cfg_attr(
        not(feature = "rust"),
        allow(dead_code, reason = "only the Rust adapter waits for a server status")
    )]
    QuiescentServerStatus { timeout: Duration },
}

/// Produces semantic facts during one language-server session.
///
/// The session validates all required server capabilities before it opens a
/// document or sends a document request.
///
/// # Errors
///
/// Returns an error if setup, capture, validation, or normalization fails.
pub fn produce_semantic_facts(
    config: &SessionConfig<'_>,
    sources: &Sources<'_>,
) -> Result<Vec<SemanticFact>, LspError> {
    let root = fs::canonicalize(config.root).map_err(|error| {
        LspError::new(format!(
            "cannot resolve the {} codebase root {}: {error}",
            language_title(config.language),
            config.root.display()
        ))
    })?;
    let root_uri = file_uri(&root)?;
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace")
        .to_owned();
    let mut client = LspClient::start(&config.server.program, &config.server.arguments)?;
    let result = client.initialize(
        initialize_parameters(
            root_uri,
            root_name,
            std::process::id(),
            config.server.initialization_options.clone(),
        ),
        INITIALIZE_TIMEOUT,
    )?;
    let captured = validate_capabilities(&result.capabilities, config.required_capabilities)
        .and_then(|()| {
            await_readiness(&mut client, config)?;
            capture_document_semantics(&mut client, config, &root, sources)
        });
    let shutdown = client.shutdown();

    let captured = captured?;
    shutdown?;
    capture::validate_capture(&captured)?;
    capture::normalize_capture(&root, sources, captured, config.fallback)
}

#[allow(
    deprecated,
    reason = "Some servers still use rootUri to find the workspace root."
)]
fn initialize_parameters(
    root_uri: Uri,
    root_name: String,
    process_id: u32,
    initialization_options: serde_json::Value,
) -> InitializeParams {
    InitializeParams {
        process_id: Some(process_id),
        root_uri: Some(root_uri.clone()),
        initialization_options: Some(initialization_options),
        capabilities: ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                call_hierarchy: Some(DynamicRegistrationClientCapabilities {
                    dynamic_registration: Some(false),
                }),
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    symbol_kind: Some(SymbolKindCapability {
                        value_set: Some(capture::supported_symbol_kinds()),
                    }),
                    hierarchical_document_symbol_support: Some(true),
                    ..DocumentSymbolClientCapabilities::default()
                }),
                references: Some(DynamicRegistrationClientCapabilities {
                    dynamic_registration: Some(false),
                }),
                type_definition: Some(GotoCapability {
                    dynamic_registration: Some(false),
                    link_support: Some(true),
                }),
                ..TextDocumentClientCapabilities::default()
            }),
            // rust-analyzer reports readiness only when the client asks.
            experimental: Some(json!({ "serverStatusNotification": true })),
            ..ClientCapabilities::default()
        },
        // Send the workspace list without workspace-folder capability support.
        // Pyright stalls when both are present.
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: root_name,
        }]),
        ..InitializeParams::default()
    }
}

const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// Waits until the server states readiness for semantic requests.
///
/// An error health fails the session, because the workspace did not load and
/// semantic answers would be empty or wrong.
///
/// # Errors
///
/// Returns an error if the server reports error health or reports no
/// quiescent status within the configured timeout.
fn await_readiness(client: &mut LspClient, config: &SessionConfig<'_>) -> Result<(), LspError> {
    let ServerReadiness::QuiescentServerStatus { timeout } = config.readiness else {
        return Ok(());
    };
    let title = language_title(config.language);
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        let status = client
            .wait_for_notification(SERVER_STATUS_METHOD, remaining)
            .map_err(|error| {
                LspError::new(format!(
                    "the {title} language server reported no ready status: {error}"
                ))
            })?;

        if status.get("health").and_then(Value::as_str) == Some("error") {
            let detail = status
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no detail");
            return Err(LspError::new(format!(
                "the {title} language server reported an error status: {detail}"
            )));
        }
        if status.get("quiescent").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
    }
}

fn validate_capabilities(
    capabilities: &ServerCapabilities,
    policy: CapabilityPolicy,
) -> Result<(), LspError> {
    if policy.requires(RequiredCapability::DocumentSymbols)
        && !matches!(
            capabilities.document_symbol_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        )
    {
        return Err(LspError::new(
            "the language server does not support document symbols",
        ));
    }
    if policy.requires(RequiredCapability::CallHierarchy)
        && !matches!(
            capabilities.call_hierarchy_provider,
            Some(
                CallHierarchyServerCapability::Simple(true)
                    | CallHierarchyServerCapability::Options(_)
            )
        )
    {
        return Err(LspError::new(
            "the language server does not support call hierarchy",
        ));
    }
    if policy.requires(RequiredCapability::References)
        && !matches!(
            capabilities.references_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        )
    {
        return Err(LspError::new(
            "the language server does not support references",
        ));
    }
    if policy.requires(RequiredCapability::TypeDefinitions)
        && !matches!(
            capabilities.type_definition_provider,
            Some(
                TypeDefinitionProviderCapability::Simple(true)
                    | TypeDefinitionProviderCapability::Options(_)
            )
        )
    {
        return Err(LspError::new(
            "the language server does not support type definitions",
        ));
    }

    Ok(())
}

/// Opens all documents, then captures facts for every hierarchical symbol.
fn capture_document_semantics(
    client: &mut LspClient,
    config: &SessionConfig<'_>,
    root: &Path,
    sources: &Sources<'_>,
) -> Result<SemanticCapture, LspError> {
    let mut files = Vec::with_capacity(sources.len());

    for document in sources.iter() {
        let path = capture::source_path(document)?;
        let uri = file_uri(&root.join(path))?;
        client
            .notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    config.language_id.to_owned(),
                    1,
                    document.content.clone(),
                ),
            })
            .map_err(|error| LspError::new(format!("cannot open {path}: {error}")))?;
        files.push((path.to_owned(), uri));
    }

    let files = files
        .into_iter()
        .enumerate()
        .map(|(index, (path, uri))| {
            let timeout = if index == 0 {
                FIRST_REQUEST_TIMEOUT
            } else {
                SYMBOL_REQUEST_TIMEOUT
            };
            let response = client
                .request::<DocumentSymbolRequest>(
                    DocumentSymbolParams {
                        text_document: TextDocumentIdentifier::new(uri.clone()),
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                    },
                    timeout,
                )
                .map_err(|error| {
                    LspError::new(format!(
                        "cannot read {} symbols from {path}: {error}",
                        language_title(config.language)
                    ))
                })?;

            Ok(CapturedDocument {
                symbols: capture::nested_symbols(response, &path)?,
                path,
                uri,
            })
        })
        .collect::<Result<Vec<_>, LspError>>()?;

    let facts = capture::request_facts(client, &files)?;

    Ok(SemanticCapture {
        documents: files,
        facts,
    })
}

const fn language_title(language: Language) -> &'static str {
    match language {
        Language::Python => "Python",
        Language::Rust => "Rust",
    }
}

const INITIALIZE_TIMEOUT: Duration = Duration::from_mins(2);
const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_mins(5);
const SYMBOL_REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        process::{self, Command},
        sync::atomic::{AtomicU64, Ordering},
    };

    use lsp_types::{OneOf, ServerCapabilities, TypeDefinitionProviderCapability};
    use serde_json::json;

    use std::time::Duration;

    use super::{
        ServerReadiness, SessionConfig, initialize_parameters, produce_semantic_facts,
        validate_capabilities,
    };
    use crate::analysis::{
        fact::ResolutionFallback,
        language::{CapabilityPolicy, Language, ServerCommand},
        lsp::ProtocolSymbolSite,
        source::{DocumentSnapshot, Sources},
    };

    #[test]
    fn initializes_with_all_captured_semantic_capabilities() {
        let root_uri = "file:///workspace"
            .parse()
            .expect("the root URI should be valid");
        let parameters = initialize_parameters(
            root_uri,
            "workspace".to_owned(),
            42,
            json!({"analysis": {"mode": "test"}}),
        );
        let actual = serde_json::to_value(parameters).expect("the parameters should serialize");

        assert_eq!(
            actual,
            json!({
                "processId": 42,
                "rootUri": "file:///workspace",
                "initializationOptions": {"analysis": {"mode": "test"}},
                "capabilities": {
                    "experimental": {"serverStatusNotification": true},
                    "textDocument": {
                        "callHierarchy": {"dynamicRegistration": false},
                        "documentSymbol": {
                            "symbolKind": {"valueSet": (1..=26).collect::<Vec<_>>()},
                            "hierarchicalDocumentSymbolSupport": true,
                        },
                        "references": {"dynamicRegistration": false},
                        "typeDefinition": {
                            "dynamicRegistration": false,
                            "linkSupport": true,
                        },
                    },
                },
                "workspaceFolders": [{
                    "uri": "file:///workspace",
                    "name": "workspace",
                }],
            })
        );
    }

    #[test]
    fn rejects_a_server_without_call_hierarchy() {
        let capabilities = ServerCapabilities {
            document_symbol_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
            ..ServerCapabilities::default()
        };

        let error = validate_capabilities(&capabilities, CapabilityPolicy::SEMANTIC_GRAPH)
            .expect_err("call hierarchy should be required");

        assert_eq!(
            error.to_string(),
            "the language server does not support call hierarchy"
        );
    }

    #[test]
    fn validates_only_capabilities_required_by_the_supplied_policy() {
        let capabilities = ServerCapabilities::default();

        validate_capabilities(&capabilities, CapabilityPolicy::NONE)
            .expect("the policy should not require server capabilities");
        validate_capabilities(&capabilities, CapabilityPolicy::SEMANTIC_GRAPH)
            .expect_err("the semantic graph policy should require capabilities");
    }

    #[test]
    fn captures_facts_with_a_shared_session() {
        let server = FakeServer::new();
        let command = server.command("serve");
        let source = DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: "def run():\n    pass\n".to_owned(),
        };
        let sources = Sources::new(vec![&source]);
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        let facts = produce_semantic_facts(&config, &sources)
            .expect("the fake language server should complete");

        assert_eq!(facts, Vec::new());
    }

    #[test]
    fn includes_server_standard_error_when_startup_fails() {
        let server = FakeServer::new();
        let command = server.command("fail");
        let sources = Sources::new(Vec::new());
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the fake language server should fail");

        assert!(
            error.to_string().contains("invalid project configuration"),
            "the error should include server standard error: {error}"
        );
    }

    #[test]
    fn does_not_request_shutdown_after_initialize_rejection() {
        let server = FakeServer::new();
        let command = server.command("reject");
        let sources = Sources::new(Vec::new());
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        produce_semantic_facts(&config, &sources)
            .expect_err("the fake language server should reject initialization");

        assert!(
            !server.shutdown_marker().exists(),
            "the client must not request shutdown after initialization fails"
        );
    }

    #[test]
    fn includes_server_standard_error_with_initialize_rejection() {
        let server = FakeServer::new();
        let command = server.command("reject");
        let sources = Sources::new(Vec::new());
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the fake language server should reject initialization");
        let message = error.to_string();

        assert!(
            message.contains("the language server returned an error: invalid initialization"),
            "the error should retain the protocol rejection: {message}"
        );
        assert!(
            message.contains("rejected project configuration"),
            "the error should include server standard error: {message}"
        );
    }

    #[test]
    fn returns_shutdown_failure_after_successful_capture() {
        let server = FakeServer::new();
        let command = server.command("shutdown-error");
        let source = DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: "def run():\n    pass\n".to_owned(),
        };
        let sources = Sources::new(vec![&source]);
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the fake language server should reject shutdown");

        assert_eq!(
            error.to_string(),
            "the language server returned an error: shutdown failed"
        );
    }

    #[test]
    fn preserves_capture_failure_when_shutdown_also_fails() {
        let server = FakeServer::new();
        let command = server.command("capture-and-shutdown-error");
        let source = DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: "def run():\n    pass\n".to_owned(),
        };
        let sources = Sources::new(vec![&source]);
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the fake language server should reject capture and shutdown");
        let message = error.to_string();

        assert!(
            message.contains("document symbol capture failed"),
            "the capture error must have precedence: {message}"
        );
        assert!(
            !message.contains("shutdown failed"),
            "the shutdown error must not replace the capture error: {message}"
        );
    }

    #[test]
    fn includes_server_standard_error_with_semantic_rejection() {
        let server = FakeServer::new();
        let command = server.command("semantic-reject-stderr");
        let source = DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: "def run():\n    pass\n".to_owned(),
        };
        let sources = Sources::new(vec![&source]);
        let config = SessionConfig {
            root: server.root(),
            language: Language::Python,
            language_id: "python",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::Immediate,
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the fake language server should reject semantic capture");
        let message = error.to_string();

        assert!(
            message.contains("document symbol capture failed"),
            "the error should retain the protocol rejection: {message}"
        );
        assert!(
            message.contains("semantic request diagnostic"),
            "the error should include server standard error: {message}"
        );
        assert_eq!(
            message.matches("language server standard error:").count(),
            1,
            "the error should include standard error once: {message}"
        );
    }

    #[test]
    fn waits_for_a_quiescent_server_status_before_capture() {
        let server = FakeServer::new();
        let command = server.command("status-quiescent");
        let source = DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: "def run():\n    pass\n".to_owned(),
        };
        let sources = Sources::new(vec![&source]);
        let config = SessionConfig {
            root: server.root(),
            language: Language::Rust,
            language_id: "rust",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::QuiescentServerStatus {
                timeout: Duration::from_secs(10),
            },
        };

        let facts = produce_semantic_facts(&config, &sources)
            .expect("the session should proceed after the quiescent status");

        assert_eq!(facts, Vec::new());
    }

    #[test]
    fn fails_when_the_server_reports_an_error_status() {
        let server = FakeServer::new();
        let command = server.command("status-error");
        let sources = Sources::new(Vec::new());
        let config = SessionConfig {
            root: server.root(),
            language: Language::Rust,
            language_id: "rust",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::QuiescentServerStatus {
                timeout: Duration::from_secs(10),
            },
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the error status should fail the session");

        assert_eq!(
            error.to_string(),
            "the Rust language server reported an error status: workspace failed to load"
        );
    }

    #[test]
    fn fails_when_the_server_reports_no_quiescent_status() {
        let server = FakeServer::new();
        let command = server.command("status-silent");
        let sources = Sources::new(Vec::new());
        let config = SessionConfig {
            root: server.root(),
            language: Language::Rust,
            language_id: "rust",
            server: &command,
            required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            fallback,
            readiness: ServerReadiness::QuiescentServerStatus {
                timeout: Duration::from_millis(300),
            },
        };

        let error = produce_semantic_facts(&config, &sources)
            .expect_err("the silent server should time out");

        assert!(
            error
                .to_string()
                .contains("the Rust language server reported no ready status"),
            "the error should name the missing status: {error}"
        );
    }

    fn fallback(_: &ProtocolSymbolSite) -> ResolutionFallback {
        ResolutionFallback::ScopeOwner
    }

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

        fn root(&self) -> &Path {
            self.directory.path()
        }

        pub(crate) fn command(&self, mode: &str) -> ServerCommand {
            ServerCommand {
                program: self.executable.to_string_lossy().into_owned(),
                arguments: vec![mode.to_owned()],
                initialization_options: json!({"analysis": {"mode": "test"}}),
            }
        }

        fn shutdown_marker(&self) -> PathBuf {
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

    const FAKE_SERVER: &str = r##"
use std::{
    env, fs,
    io::{self, BufRead, Write},
    process,
};

fn main() -> io::Result<()> {
    let mode = env::args().nth(1).unwrap_or_default();
    if mode == "fail" {
        eprintln!("invalid project configuration");
        process::exit(1);
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();

    while let Some(message) = read_message(&mut input)? {
        let request_id = request_id(&message);

        if message.contains(r#""method":"initialize""#) {
            let request_id = required_id(request_id)?;
            if mode == "reject" {
                eprintln!("rejected project configuration");
                write_error(&mut output, request_id, "invalid initialization")?;
                continue;
            }
            if !message.contains(
                r#""initializationOptions":{"analysis":{"mode":"test"}}"#,
            ) {
                eprintln!("initialization options were not forwarded");
                process::exit(1);
            }
            write_result(
                &mut output,
                request_id,
                r#"{"capabilities":{"documentSymbolProvider":true,"callHierarchyProvider":true,"referencesProvider":true,"typeDefinitionProvider":true}}"#,
            )?;
        } else if message.contains(r#""method":"initialized""#) {
            // A well-behaved server reports readiness when the client asks.
            // The status modes replace that report to test the waiting client.
            if mode == "status-quiescent" {
                write_notification(
                    &mut output,
                    "experimental/serverStatus",
                    r#"{"health":"ok","quiescent":false}"#,
                )?;
                write_notification(
                    &mut output,
                    "experimental/serverStatus",
                    r#"{"health":"ok","quiescent":true}"#,
                )?;
            } else if mode == "status-error" {
                write_notification(
                    &mut output,
                    "experimental/serverStatus",
                    r#"{"health":"error","quiescent":false,"message":"workspace failed to load"}"#,
                )?;
            } else if mode != "status-silent" {
                write_notification(
                    &mut output,
                    "experimental/serverStatus",
                    r#"{"health":"ok","quiescent":true}"#,
                )?;
            }
        } else if message.contains(r#""method":"textDocument/documentSymbol""#) {
            let request_id = required_id(request_id)?;
            if mode == "protocol-error" || mode == "capture-and-shutdown-error" {
                write_error(&mut output, request_id, "document symbol capture failed")?;
                continue;
            }
            if mode == "semantic-reject-stderr" {
                eprintln!("semantic request diagnostic");
                write_error(&mut output, request_id, "document symbol capture failed")?;
                continue;
            }
            write_result(
                &mut output,
                request_id,
                r#"[{"name":"run","kind":12,"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":8}},"selectionRange":{"start":{"line":0,"character":4},"end":{"line":0,"character":7}}}]"#,
            )?;
        } else if message.contains(r#""method":"textDocument/prepareCallHierarchy""#)
            || message.contains(r#""method":"textDocument/references""#)
        {
            write_result(&mut output, required_id(request_id)?, "[]")?;
        } else if message.contains(r#""method":"textDocument/typeDefinition""#) {
            write_result(&mut output, required_id(request_id)?, "null")?;
        } else if message.contains(r#""method":"shutdown""#) {
            let executable = env::current_exe()?;
            let directory = executable.parent().ok_or_else(|| {
                io::Error::other("the fake server executable has no parent")
            })?;
            fs::write(directory.join("shutdown-requested"), "requested")?;
            if mode == "shutdown-error" || mode == "capture-and-shutdown-error" {
                write_error(&mut output, required_id(request_id)?, "shutdown failed")?;
                continue;
            }
            write_result(&mut output, required_id(request_id)?, "null")?;
        } else if message.contains(r#""method":"exit""#) {
            break;
        }
    }

    Ok(())
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, error)
            })?);
        }
    }

    let length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length")
    })?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn request_id(message: &str) -> Option<&str> {
    let value = message.split_once(r#""id":"#)?.1.trim_start();
    let length = value.bytes().take_while(u8::is_ascii_digit).count();
    (length > 0).then_some(&value[..length])
}

fn required_id(request_id: Option<&str>) -> io::Result<&str> {
    request_id.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request id"))
}

fn write_result(writer: &mut impl Write, request_id: &str, result: &str) -> io::Result<()> {
    write_message(
        writer,
        format!(r#"{{"jsonrpc":"2.0","id":{request_id},"result":{result}}}"#),
    )
}

fn write_notification(writer: &mut impl Write, method: &str, params: &str) -> io::Result<()> {
    write_message(
        writer,
        format!(r#"{{"jsonrpc":"2.0","method":{method:?},"params":{params}}}"#),
    )
}

fn write_error(writer: &mut impl Write, request_id: &str, message: &str) -> io::Result<()> {
    write_message(
        writer,
        format!(
            r#"{{"jsonrpc":"2.0","id":{request_id},"error":{{"code":-32002,"message":{message:?}}}}}"#
        ),
    )
}

fn write_message(writer: &mut impl Write, body: String) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    writer.flush()
}
"##;
}
