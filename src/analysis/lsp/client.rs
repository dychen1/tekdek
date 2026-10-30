//! Language server client.
//!
//! This module owns the server process, message transport, and file URI
//! conversion. A server command, protocol values, and paths are the inputs.
//! Protocol results and file URIs are the outputs.
//!
//! # Sections
//!
//! `Client` owns the server process and synchronous requests.
//! `Message Framing` reads and writes LSP messages.
//! `Message Classification` identifies notifications, requests, and responses.
//! `File URIs` converts file paths to file URIs and back.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use lsp_types::{
    InitializeParams, InitializeResult, InitializedParams, Uri,
    notification::{Exit, Initialized, Notification},
    request::{Initialize, Request, Shutdown},
};
use serde_json::{Value, json};
use url::Url;

use super::LspError;
use crate::analysis::{program::find_program, timing};

// -----------------------------------------------------------------------------
// Client
// -----------------------------------------------------------------------------

pub struct LspClient {
    process: Child,
    stdin: ChildStdin,
    incoming_messages: Receiver<Result<Value, LspError>>,
    reader_thread: Option<JoinHandle<()>>,
    standard_error: Arc<Mutex<Vec<u8>>>,
    standard_error_thread: Option<JoinHandle<()>>,
    request_id_counter: u64,
}

impl LspClient {
    /// # Errors
    ///
    /// Returns an error if the process or reader thread fails to start.
    pub fn start(program: &str, arguments: &[String]) -> Result<Self, LspError> {
        // Start the resolved file, so a Windows `.cmd` wrapper on PATH also starts.
        let executable = find_program(program).unwrap_or_else(|| PathBuf::from(program));
        let mut process = Command::new(executable)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| LspError::new(format!("cannot start {program}: {error}")))?;

        let Some(stdin) = process.stdin.take() else {
            stop_process(&mut process);
            return Err(LspError::new("the language server has no input"));
        };
        let Some(stdout) = process.stdout.take() else {
            stop_process(&mut process);
            return Err(LspError::new("the language server has no output"));
        };
        let Some(stderr) = process.stderr.take() else {
            stop_process(&mut process);
            return Err(LspError::new("the language server has no standard error"));
        };

        let standard_error = Arc::new(Mutex::new(Vec::new()));
        let standard_error_capture = Arc::clone(&standard_error);
        let standard_error_thread = match thread::Builder::new()
            .name("lsp-stderr-reader".to_owned())
            .spawn(move || read_standard_error(stderr, &standard_error_capture))
        {
            Ok(thread) => thread,
            Err(error) => {
                stop_process(&mut process);
                return Err(LspError::new(format!(
                    "cannot start the language server standard-error reader: {error}"
                )));
            }
        };
        let (sender, incoming_messages) = mpsc::channel();
        let reader_thread = match thread::Builder::new()
            .name("lsp-reader".to_owned())
            .spawn(move || read_loop(BufReader::new(stdout), &sender))
        {
            Ok(thread) => thread,
            Err(error) => {
                stop_process(&mut process);
                let _ = standard_error_thread.join();
                return Err(append_standard_error(
                    LspError::new(format!(
                        "cannot start the language server output reader: {error}"
                    )),
                    &standard_error,
                ));
            }
        };

        Ok(Self {
            process,
            stdin,
            incoming_messages,
            reader_thread: Some(reader_thread),
            standard_error,
            standard_error_thread: Some(standard_error_thread),
            request_id_counter: 0,
        })
    }

    /// # Errors
    ///
    /// Returns an error if parameter encoding or the transport write fails.
    pub fn notify<N>(&mut self, parameters: N::Params) -> Result<(), LspError>
    where
        N: Notification,
    {
        let parameters = serde_json::to_value(parameters).map_err(|error| {
            LspError::new(format!(
                "cannot encode the {} parameters: {error}",
                N::METHOD
            ))
        })?;

        self.send(&json!({
            "jsonrpc": "2.0",
            "method": N::METHOD,
            "params": parameters,
        }))
        .map_err(|error| self.with_standard_error(error))
    }

    /// # Errors
    ///
    /// Returns an error if JSON conversion or transport fails, the server rejects the request,
    /// or the timeout expires.
    pub fn request<R>(
        &mut self,
        parameters: R::Params,
        timeout: Duration,
    ) -> Result<R::Result, LspError>
    where
        R: Request,
    {
        let parameters = serde_json::to_value(parameters).map_err(|error| {
            LspError::new(format!(
                "cannot encode the {} parameters: {error}",
                R::METHOD
            ))
        })?;
        let started = timing::start();
        let result = self.request_value(R::METHOD, &parameters, timeout);
        timing::record_request(started, R::METHOD);
        result
            .and_then(|result| {
                serde_json::from_value(result).map_err(|error| {
                    LspError::new(format!("cannot decode the {} result: {error}", R::METHOD))
                })
            })
            .map_err(|error| self.with_standard_error(error))
    }

    /// Sends many requests with at most `window` in flight, and returns the
    /// results in input order.
    ///
    /// The batch times out when no request gets an answer for `timeout`, so a
    /// server that answers one request after the other never times out a
    /// queued request. Server requests that arrive in between are answered,
    /// and other messages are skipped. After the first error, the batch sends
    /// nothing more.
    ///
    /// # Errors
    ///
    /// Returns the input index and the error of the first request that fails
    /// or cannot be sent. A timeout names the oldest unanswered request.
    pub fn request_batch(
        &mut self,
        requests: &[(&'static str, Value)],
        window: usize,
        timeout: Duration,
    ) -> Result<Vec<Value>, (usize, LspError)> {
        self.batch_values(requests, window.max(1), timeout)
            .map_err(|(index, error)| (index, self.with_standard_error(error)))
    }

    /// Sends the initialization request and then the initialized notification.
    ///
    /// # Errors
    ///
    /// Returns an error if the initialization request or notification fails.
    pub fn initialize(
        &mut self,
        parameters: InitializeParams,
        timeout: Duration,
    ) -> Result<InitializeResult, LspError> {
        let result = self.request::<Initialize>(parameters, timeout)?;
        self.notify::<Initialized>(InitializedParams {})?;
        Ok(result)
    }

    /// Waits for one notification with the given method and returns its
    /// parameters.
    ///
    /// The wait answers server requests and discards every other message.
    ///
    /// # Errors
    ///
    /// Returns an error if the timeout expires or the transport fails.
    pub fn wait_for_notification(
        &mut self,
        method: &str,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        self.notification_value(method, timeout)
            .map_err(|error| self.with_standard_error(error))
    }

    /// Requests a clean server shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if the shutdown request, exit notification, or process check fails.
    pub fn shutdown(mut self) -> Result<(), LspError> {
        self.request::<Shutdown>((), Duration::from_secs(10))?;
        self.notify::<Exit>(())?;
        self.wait_for_exit(Duration::from_secs(5))
    }

    /// Answers server requests and skips other messages until the notification
    /// arrives. One timeout covers the full wait loop.
    fn notification_value(&mut self, method: &str, timeout: Duration) -> Result<Value, LspError> {
        let deadline = Instant::now() + timeout;

        loop {
            let message = self.receive(method, deadline)?;

            if is_server_request(&message) {
                self.answer_server_request(&message)?;
                continue;
            }

            if message.get("id").is_none()
                && message.get("method").and_then(Value::as_str) == Some(method)
            {
                return Ok(message.get("params").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// Answers server requests that arrive before the matching response.
    /// Discards unmatched responses. One timeout covers the full wait loop.
    fn request_value(
        &mut self,
        method: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let request_id = self.allocate_request_id();

        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": parameters,
        }))?;

        let deadline = Instant::now() + timeout;

        loop {
            let message = self.receive(method, deadline)?;

            if is_server_request(&message) {
                self.answer_server_request(&message)?;
                continue;
            }

            if is_response_to(&message, request_id) {
                return response_result(&message);
            }
        }
    }

    /// Keeps up to `window` requests in flight and matches answers by ID.
    fn batch_values(
        &mut self,
        requests: &[(&'static str, Value)],
        window: usize,
        timeout: Duration,
    ) -> Result<Vec<Value>, (usize, LspError)> {
        let mut results = vec![Value::Null; requests.len()];
        let mut unsent = requests.iter().enumerate();
        let mut in_flight = BTreeMap::<u64, InFlight>::new();
        let mut deadline = Instant::now() + timeout;

        loop {
            while in_flight.len() < window {
                let Some((index, (method, parameters))) = unsent.next() else {
                    break;
                };
                let request_id = self.allocate_request_id();
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": method,
                    "params": parameters,
                }))
                .map_err(|error| (index, error))?;
                in_flight.insert(
                    request_id,
                    InFlight {
                        index,
                        method,
                        started: timing::start(),
                    },
                );
            }
            let Some((_, oldest)) = in_flight.first_key_value() else {
                return Ok(results);
            };
            let (oldest_index, oldest_method) = (oldest.index, oldest.method);

            let message = self
                .receive(oldest_method, deadline)
                .map_err(|error| (oldest_index, error))?;
            if is_server_request(&message) {
                self.answer_server_request(&message)
                    .map_err(|error| (oldest_index, error))?;
                continue;
            }
            let Some(request) = message
                .get("id")
                .and_then(Value::as_u64)
                .and_then(|request_id| in_flight.remove(&request_id))
            else {
                continue;
            };
            deadline = Instant::now() + timeout;
            timing::record_request(request.started, request.method);
            results[request.index] =
                response_result(&message).map_err(|error| (request.index, error))?;
        }
    }

    fn allocate_request_id(&mut self) -> u64 {
        self.request_id_counter += 1;
        self.request_id_counter
    }

    fn send(&mut self, message: &Value) -> Result<(), LspError> {
        write_message(&mut self.stdin, message)
    }

    /// Waits for the server process to exit and kills it after the timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if the operating system cannot read the process state.
    fn wait_for_exit(&mut self, timeout: Duration) -> Result<(), LspError> {
        let deadline = Instant::now() + timeout;

        while self.process.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                let _ = self.process.kill();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        Ok(())
    }

    /// Waits for the next message from the background reader.
    ///
    /// # Errors
    ///
    /// Returns an error if the deadline expires or the reader stops.
    fn receive(&mut self, method: &str, deadline: Instant) -> Result<Value, LspError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| LspError::timeout(method))?;

        match self.incoming_messages.recv_timeout(remaining) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(LspError::timeout(method)),
            Err(RecvTimeoutError::Disconnected) => {
                Err(LspError::new("the language server reader stopped"))
            }
        }
    }

    fn answer_server_request(&mut self, message: &Value) -> Result<(), LspError> {
        self.send(&server_request_response(message))
    }

    /// Adds the retained server standard error to one failure.
    ///
    /// The public methods and the batch decoder of `capture` call this once
    /// for each error. The wait lets a server that stops at the failure write
    /// its last lines.
    pub(crate) fn with_standard_error(&mut self, error: LspError) -> LspError {
        let deadline = Instant::now() + STDERR_EXIT_WAIT;

        loop {
            match self.process.try_wait() {
                Ok(Some(_)) => {
                    if let Some(thread) = self.standard_error_thread.take() {
                        let _ = thread.join();
                    }
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(None) | Err(_) => break,
            }
        }

        append_standard_error(error, &self.standard_error)
    }
}

/// One batch request that waits for its answer.
struct InFlight {
    index: usize,
    method: &'static str,
    started: Option<Instant>,
}

impl Drop for LspClient {
    fn drop(&mut self) {
        stop_process(&mut self.process);

        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        if let Some(standard_error_thread) = self.standard_error_thread.take() {
            let _ = standard_error_thread.join();
        }
    }
}

fn stop_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

const STDERR_CAPTURE_LIMIT: usize = 64 * 1024;
const STDERR_EXIT_WAIT: Duration = Duration::from_millis(50);

/// Drains server standard error and retains a bounded prefix.
fn read_standard_error(mut reader: impl Read, capture: &Mutex<Vec<u8>>) {
    let mut buffer = [0; 8 * 1024];

    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let mut retained = match capture.lock() {
            Ok(retained) => retained,
            Err(poisoned) => poisoned.into_inner(),
        };
        let available = STDERR_CAPTURE_LIMIT.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..count.min(available)]);
    }
}

fn append_standard_error(error: LspError, capture: &Mutex<Vec<u8>>) -> LspError {
    let retained = match capture.lock() {
        Ok(retained) => retained,
        Err(poisoned) => poisoned.into_inner(),
    };
    let standard_error = String::from_utf8_lossy(&retained);
    let standard_error = standard_error.trim();

    if standard_error.is_empty() {
        error
    } else {
        LspError::new(format!("{error}\n{STDERR_ERROR_HEADING}\n{standard_error}"))
    }
}

const STDERR_ERROR_HEADING: &str = "language server standard error:";

/// Returns the response for one server request.
///
/// The client answers `workspace/configuration` with one empty object for
/// each requested item. The client rejects every other server request.
fn server_request_response(message: &Value) -> Value {
    if message.get("method").and_then(Value::as_str) == Some("workspace/configuration") {
        let item_count = message
            .pointer("/params/items")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);

        return json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": vec![json!({}); item_count],
        });
    }

    json!({
        "jsonrpc": "2.0",
        "id": message["id"],
        "error": {
            "code": -32601,
            "message": "Method not found",
        },
    })
}

// -----------------------------------------------------------------------------
// Message Framing
// -----------------------------------------------------------------------------

/// Reads LSP messages from the server output.
///
/// Sends every message, notifications included, and the first read error
/// through the channel. Each receive loop skips the messages it does not wait
/// for. Stops after an error, a closed output, or a closed channel.
fn read_loop(mut reader: impl BufRead, sender: &Sender<Result<Value, LspError>>) {
    loop {
        match read_message(&mut reader) {
            Ok(Some(message)) => {
                if sender.send(Ok(message)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(Err(LspError::new("the language server closed its output")));
                return;
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        }
    }
}

/// Writes one LSP message to the server input.
///
/// Serializes JSON, writes the required header and body, and flushes the writer.
///
/// A `null` `params` member is left out, as JSON-RPC allows: TypeScript 7
/// rejects `"params": null` on `shutdown`.
///
/// # Errors
///
/// Returns an error if JSON serialization or a writer operation fails.
fn write_message(writer: &mut impl Write, message: &Value) -> Result<(), LspError> {
    let body = if message.get("params").is_some_and(Value::is_null) {
        let mut message = message.clone();
        if let Some(object) = message.as_object_mut() {
            object.remove("params");
        }
        serde_json::to_vec(&message)?
    } else {
        serde_json::to_vec(message)?
    };

    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;

    Ok(())
}

/// Reads one LSP message from the server output.
///
/// An empty line ends the headers. `Content-Length` specifies the body size.
/// Returns `Ok(None)` if the server closes its output during a header read.
///
/// # Errors
///
/// Returns an error if the reader fails.
/// Returns an error if the message has no valid `Content-Length` header.
/// Returns an error if the body contains invalid JSON.
fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, LspError> {
    let mut content_length = None;

    loop {
        let mut line = String::new();

        // `read_line` waits until it reads a line or the server closes its output.
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }

        if line == "\r\n" || line == "\n" {
            break;
        }

        let Some((name, value)) = line.split_once(':') else {
            continue;
        };

        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| LspError::new("the LSP content length is invalid"))?,
            );
        }
    }

    let length =
        content_length.ok_or_else(|| LspError::new("the LSP message has no content length"))?;
    let mut body = vec![0; length];

    reader.read_exact(&mut body)?;

    Ok(Some(serde_json::from_slice(&body)?))
}

// -----------------------------------------------------------------------------
// Message Classification
// -----------------------------------------------------------------------------

fn is_server_request(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}

fn is_response_to(message: &Value, request_id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(request_id)
}

/// Returns the result value of one response.
///
/// A response without a result and without an error yields a null value.
///
/// # Errors
///
/// Returns an error when the response carries a server error.
fn response_result(message: &Value) -> Result<Value, LspError> {
    if let Some(error) = message.get("error") {
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| error.to_string(), str::to_owned);

        return Err(LspError::new(format!(
            "the language server returned an error: {detail}"
        )));
    }

    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

// -----------------------------------------------------------------------------
// File URIs
// -----------------------------------------------------------------------------

/// Converts an absolute file path to an LSP file URI.
///
/// # Errors
///
/// Returns an error if the path is relative or file URL conversion fails.
/// Path characters that a file URL keeps but an LSP URI rejects, such as
/// the brackets of a Next.js route folder, are percent-encoded.
pub fn file_uri(path: &Path) -> Result<Uri, LspError> {
    let mut url = Url::from_file_path(path).map_err(|()| {
        LspError::new(format!(
            "the LSP document path must be absolute: {}",
            path.display()
        ))
    })?;
    let encoded = url
        .path()
        .chars()
        .fold(String::new(), |mut text, character| {
            match character {
                '[' => text.push_str("%5B"),
                ']' => text.push_str("%5D"),
                '^' => text.push_str("%5E"),
                '|' => text.push_str("%7C"),
                _ => text.push(character),
            }
            text
        });
    url.set_path(&encoded);

    url.as_str().parse().map_err(|error| {
        LspError::new(format!(
            "cannot create an LSP file URI for {}: {error}",
            path.display()
        ))
    })
}

/// Returns the absolute file path that one LSP file URI names.
///
/// Percent decoding lets equivalent URI spellings return the same path.
/// A URI with another scheme returns `None`.
#[must_use]
pub fn uri_path(uri: &Uri) -> Option<PathBuf> {
    Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        io::{BufReader, Cursor},
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use serde_json::json;

    use super::{
        LspClient, STDERR_CAPTURE_LIMIT, file_uri, read_message, read_standard_error,
        server_request_response, uri_path, write_message,
    };
    use crate::analysis::lsp::fake_server::FakeServer;

    #[test]
    fn returns_batch_answers_in_input_order_and_answers_server_requests() {
        let server = FakeServer::new();
        let mut client = echo_client(&server, "batch");

        let results = client
            .request_batch(&echo_requests(), 64, Duration::from_secs(10))
            .expect("the batch should complete");

        assert_eq!(results, [json!(1), json!(2), json!(3)]);
    }

    #[test]
    fn returns_the_index_of_a_failed_batch_request() {
        let server = FakeServer::new();
        let mut client = echo_client(&server, "batch-error");

        let (index, error) = client
            .request_batch(&echo_requests(), 64, Duration::from_secs(10))
            .expect_err("the second answer is an error");

        assert_eq!(index, 1);
        assert!(
            error.to_string().contains("echo failed"),
            "the error should name the server error: {error}"
        );
    }

    #[test]
    fn times_out_only_when_no_answer_arrives_for_the_timeout() {
        let server = FakeServer::new();
        let mut client = echo_client(&server, "batch-slow");
        // A first batch waits out the start of the new server program.
        client
            .request_batch(&echo_requests(), 64, Duration::from_secs(10))
            .expect("the first batch should complete");

        let results = client
            .request_batch(&echo_requests(), 64, Duration::from_millis(500))
            .expect("each answer arrives within the timeout of the previous one");

        assert_eq!(results, [json!(1), json!(2), json!(3)]);
    }

    #[test]
    fn keeps_no_more_requests_in_flight_than_the_window() {
        let server = FakeServer::new();
        let mut client = echo_client(&server, "window-2");

        let results = client
            .request_batch(&echo_requests(), 2, Duration::from_secs(10))
            .expect("the fake server should see at most two unanswered requests");

        assert_eq!(results, [json!(1), json!(2), json!(3)]);
    }

    fn echo_client(server: &FakeServer, mode: &str) -> LspClient {
        let command = server.command(mode);
        LspClient::start(&command.program, &command.arguments)
            .expect("the fake language server should start")
    }

    fn echo_requests() -> [(&'static str, serde_json::Value); 3] {
        [
            ("test/echo", json!(1)),
            ("test/echo", json!(2)),
            ("test/echo", json!(3)),
        ]
    }

    #[test]
    fn reads_a_written_message() {
        let expected = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"name": "pyright"},
        });
        let mut bytes = Vec::new();

        write_message(&mut bytes, &expected).expect("the message should be written");

        let mut reader = BufReader::new(Cursor::new(bytes));
        let actual = read_message(&mut reader)
            .expect("the message should be valid")
            .expect("the message should exist");

        assert_eq!(actual, expected);
    }

    #[test]
    fn leaves_out_null_params() {
        let mut bytes = Vec::new();

        write_message(
            &mut bytes,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown", "params": null}),
        )
        .expect("the message should be written");

        let mut reader = BufReader::new(Cursor::new(bytes));
        let actual = read_message(&mut reader)
            .expect("the message should be valid")
            .expect("the message should exist");

        assert_eq!(
            actual,
            json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"})
        );
    }

    #[test]
    fn rejects_a_message_without_content_length() {
        let bytes = b"Content-Type: application/json\r\n\r\n{}";
        let mut reader = BufReader::new(Cursor::new(bytes));

        assert!(read_message(&mut reader).is_err());
    }

    #[test]
    fn bounds_retained_server_standard_error() {
        let standard_error = Arc::new(Mutex::new(Vec::new()));
        let input = vec![b'x'; STDERR_CAPTURE_LIMIT + 100];

        read_standard_error(Cursor::new(input), &standard_error);

        let retained = standard_error
            .lock()
            .expect("the standard error capture should not be poisoned");
        assert_eq!(retained.len(), STDERR_CAPTURE_LIMIT);
    }

    #[test]
    fn answers_a_workspace_configuration_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "workspace/configuration",
            "params": {
                "items": [
                    {"section": "python"},
                    {"section": "python.analysis"},
                ],
            },
        });

        assert_eq!(
            server_request_response(&request),
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "result": [{}, {}],
            })
        );
    }

    #[test]
    fn rejects_an_unknown_server_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "workspace/unknown",
            "params": {},
        });

        assert_eq!(
            server_request_response(&request),
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "error": {
                    "code": -32601,
                    "message": "Method not found",
                },
            })
        );
    }

    #[test]
    fn encodes_a_file_path_as_a_uri() {
        let uri = file_uri(Path::new("/tmp/code graph/naïve.py"))
            .expect("the absolute path should form a URI");

        assert_eq!(uri.as_str(), "file:///tmp/code%20graph/na%C3%AFve.py");
    }

    #[test]
    fn encodes_route_folder_brackets_and_reads_the_path_back() {
        let path = Path::new("/tmp/app/[...slug]/a^b|c.tsx");
        let uri = file_uri(path).expect("the bracket path should form a URI");

        assert_eq!(uri.as_str(), "file:///tmp/app/%5B...slug%5D/a%5Eb%7Cc.tsx");
        assert_eq!(uri_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn rejects_a_relative_file_path() {
        assert!(file_uri(Path::new("src/main.py")).is_err());
    }

    #[test]
    fn reads_a_path_back_from_its_uri() {
        let path = Path::new("/tmp/code graph/naïve.py");
        let uri = file_uri(path).expect("the absolute path should form a URI");

        assert_eq!(uri_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn reads_one_path_from_every_uri_spelling() {
        let spellings = [
            "file:///tmp/a.b.py",
            "file:///tmp/a%2Eb.py",
            "file://localhost/tmp/a.b.py",
        ];

        for spelling in spellings {
            let uri = spelling.parse().expect("the fixture URI should parse");

            assert_eq!(
                uri_path(&uri).as_deref(),
                Some(Path::new("/tmp/a.b.py")),
                "{spelling} must name one path"
            );
        }
    }
}
