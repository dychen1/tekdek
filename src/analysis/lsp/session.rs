//! Shared language-server session.
//!
//! This module owns the server start: process start, initialization,
//! capability validation, and readiness waiting. The resolver sends the
//! requests of `capture` to the ready server and stops it. A session
//! configuration is the input. A ready client is the output.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use lsp_types::{
    CallHierarchyServerCapability, ClientCapabilities, DocumentSymbolClientCapabilities,
    DynamicRegistrationClientCapabilities, GotoCapability, InitializeParams, OneOf,
    ServerCapabilities, SymbolKindCapability, TextDocumentClientCapabilities,
    TypeDefinitionProviderCapability, Uri, WorkspaceFolder,
};
use serde_json::{Value, json};

use crate::analysis::language::{Language, ServerCommand};

use super::{FallbackPolicy, LspClient, LspError, file_uri, normalize};
use crate::analysis::timing;

pub struct SessionConfig<'a> {
    pub root: &'a Path,
    pub language: Language,
    pub language_id: &'a str,
    pub server: &'a ServerCommand,
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

/// Starts one server and returns it ready for semantic requests, with the canonical root.
///
/// A server that fails validation or readiness gets a shutdown request, and
/// the first error wins.
///
/// # Errors
///
/// Returns an error if the server cannot start, initialize, or become ready.
pub(crate) fn start(config: &SessionConfig<'_>) -> Result<(LspClient, PathBuf), LspError> {
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
    let started = timing::start();
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
    if let Err(error) = validate_capabilities(&result.capabilities)
        .and_then(|()| await_readiness(&mut client, config))
    {
        let _ = client.shutdown();
        return Err(error);
    }
    timing::record_stage(started, || format!("{} server start", config.language));
    Ok((client, root))
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
                definition: Some(GotoCapability {
                    dynamic_registration: Some(false),
                    link_support: Some(true),
                }),
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    symbol_kind: Some(SymbolKindCapability {
                        value_set: Some(normalize::supported_symbol_kinds()),
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

/// Requires the capabilities that capture uses for every symbol and every
/// imported name.
fn validate_capabilities(capabilities: &ServerCapabilities) -> Result<(), LspError> {
    if !matches!(
        capabilities.document_symbol_provider,
        Some(OneOf::Left(true) | OneOf::Right(_))
    ) {
        return Err(LspError::new(
            "the language server does not support document symbols",
        ));
    }
    if !matches!(
        capabilities.call_hierarchy_provider,
        Some(
            CallHierarchyServerCapability::Simple(true) | CallHierarchyServerCapability::Options(_)
        )
    ) {
        return Err(LspError::new(
            "the language server does not support call hierarchy",
        ));
    }
    if !matches!(
        capabilities.references_provider,
        Some(OneOf::Left(true) | OneOf::Right(_))
    ) {
        return Err(LspError::new(
            "the language server does not support references",
        ));
    }
    if !matches!(
        capabilities.type_definition_provider,
        Some(
            TypeDefinitionProviderCapability::Simple(true)
                | TypeDefinitionProviderCapability::Options(_)
        )
    ) {
        return Err(LspError::new(
            "the language server does not support type definitions",
        ));
    }
    if !matches!(
        capabilities.definition_provider,
        Some(OneOf::Left(true) | OneOf::Right(_))
    ) {
        return Err(LspError::new(
            "the language server does not support definitions",
        ));
    }

    Ok(())
}

pub(super) const fn language_title(language: Language) -> &'static str {
    match language {
        Language::Python => "Python",
        Language::Rust => "Rust",
    }
}

const INITIALIZE_TIMEOUT: Duration = Duration::from_mins(2);

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lsp_types::{
        CallHierarchyServerCapability, OneOf, ServerCapabilities, TypeDefinitionProviderCapability,
    };
    use serde_json::json;

    use super::{
        ServerReadiness, SessionConfig, initialize_parameters, start, validate_capabilities,
    };
    use crate::analysis::{
        fact::ResolutionFallback,
        language::Language,
        lsp::{LspError, ProtocolSymbolSite, fake_server::FakeServer},
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
                        "definition": {
                            "dynamicRegistration": false,
                            "linkSupport": true,
                        },
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
    fn rejects_a_server_without_document_symbols() {
        let capabilities = ServerCapabilities {
            call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
            references_provider: Some(OneOf::Left(true)),
            type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
            ..ServerCapabilities::default()
        };

        let error =
            validate_capabilities(&capabilities).expect_err("document symbols should be required");

        assert_eq!(
            error.to_string(),
            "the language server does not support document symbols"
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

        let error =
            validate_capabilities(&capabilities).expect_err("call hierarchy should be required");

        assert_eq!(
            error.to_string(),
            "the language server does not support call hierarchy"
        );
    }

    #[test]
    fn rejects_a_server_without_definitions() {
        let capabilities = ServerCapabilities {
            document_symbol_provider: Some(OneOf::Left(true)),
            call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
            references_provider: Some(OneOf::Left(true)),
            type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
            ..ServerCapabilities::default()
        };

        let error =
            validate_capabilities(&capabilities).expect_err("definitions should be required");

        assert_eq!(
            error.to_string(),
            "the language server does not support definitions"
        );
    }

    #[test]
    fn starts_and_stops_a_ready_session() {
        let server = FakeServer::new();

        run_session(&server, "serve", Language::Python, IMMEDIATE)
            .expect("the fake language server should start and stop");

        assert!(server.shutdown_marker().exists());
    }

    #[test]
    fn includes_server_standard_error_when_startup_fails() {
        let error = run_session(&FakeServer::new(), "fail", Language::Python, IMMEDIATE)
            .expect_err("the fake language server should fail");

        assert!(
            error.to_string().contains("invalid project configuration"),
            "the error should include server standard error: {error}"
        );
    }

    #[test]
    fn does_not_request_shutdown_after_initialize_rejection() {
        let server = FakeServer::new();

        run_session(&server, "reject", Language::Python, IMMEDIATE)
            .expect_err("the fake language server should reject initialization");

        assert!(
            !server.shutdown_marker().exists(),
            "the client must not request shutdown after initialization fails"
        );
    }

    #[test]
    fn includes_server_standard_error_with_initialize_rejection() {
        let error = run_session(&FakeServer::new(), "reject", Language::Python, IMMEDIATE)
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
        assert_eq!(
            message.matches("language server standard error:").count(),
            1,
            "the error should include standard error once: {message}"
        );
    }

    #[test]
    fn includes_server_standard_error_with_an_undecodable_initialize_result() {
        let error = run_session(
            &FakeServer::new(),
            "undecodable-initialize",
            Language::Python,
            IMMEDIATE,
        )
        .expect_err("the initialize result should not decode");
        let message = error.to_string();

        assert!(
            message.contains("cannot decode the initialize result"),
            "the error should name the decode failure: {message}"
        );
        assert!(
            message.contains("broken capability report"),
            "the error should include server standard error: {message}"
        );
        assert_eq!(
            message.matches("language server standard error:").count(),
            1,
            "the error should include standard error once: {message}"
        );
    }

    #[test]
    fn returns_a_shutdown_failure() {
        let error = run_session(
            &FakeServer::new(),
            "shutdown-error",
            Language::Python,
            IMMEDIATE,
        )
        .expect_err("the fake language server should reject shutdown");

        assert_eq!(
            error.to_string(),
            "the language server returned an error: shutdown failed"
        );
    }

    #[test]
    fn waits_for_a_quiescent_server_status() {
        run_session(
            &FakeServer::new(),
            "status-quiescent",
            Language::Rust,
            quiescent(Duration::from_secs(10)),
        )
        .expect("the session should proceed after the quiescent status");
    }

    #[test]
    fn fails_when_the_server_reports_an_error_status() {
        let error = run_session(
            &FakeServer::new(),
            "status-error",
            Language::Rust,
            quiescent(Duration::from_secs(10)),
        )
        .expect_err("the error status should fail the session");

        assert_eq!(
            error.to_string(),
            "the Rust language server reported an error status: workspace failed to load"
        );
    }

    #[test]
    fn fails_when_the_server_reports_no_quiescent_status() {
        let error = run_session(
            &FakeServer::new(),
            "status-silent",
            Language::Rust,
            quiescent(Duration::from_millis(300)),
        )
        .expect_err("the silent server should time out");

        assert!(
            error
                .to_string()
                .contains("the Rust language server reported no ready status"),
            "the error should name the missing status: {error}"
        );
    }

    const IMMEDIATE: ServerReadiness = ServerReadiness::Immediate;

    fn quiescent(timeout: Duration) -> ServerReadiness {
        ServerReadiness::QuiescentServerStatus { timeout }
    }

    /// Starts and stops one session against the fake server in the given mode.
    fn run_session(
        server: &FakeServer,
        mode: &str,
        language: Language,
        readiness: ServerReadiness,
    ) -> Result<(), LspError> {
        let command = server.command(mode);
        let config = SessionConfig {
            root: server.root(),
            language,
            language_id: language.as_str(),
            server: &command,
            fallback,
            readiness,
        };

        start(&config)?.0.shutdown()
    }

    fn fallback(_: &ProtocolSymbolSite) -> ResolutionFallback {
        ResolutionFallback::ScopeOwner
    }
}
