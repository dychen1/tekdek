//! Language server client.
//!
//! This module owns the server process, message transport, and path and position
//! conversion. A server command, protocol values, paths, and source text are the
//! inputs. Protocol results, file URIs, and source byte offsets are the outputs.
//!
//! # Sections
//!
//! `Client` owns the server process and synchronous requests.
//! `Message Framing` reads and writes LSP messages.
//! `Message Classification` identifies notifications, requests, and responses.
//! `Path and Position Conversion` converts file paths and source coordinates.

use std::{
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
    InitializeParams, InitializeResult, InitializedParams, Position, Uri,
    notification::{Exit, Initialized, Notification},
    request::{Initialize, Request, Shutdown},
};
use serde_json::{Value, json};
use url::Url;

use super::LspError;
use crate::analysis::program::find_program;

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
        let result = match self.request_value(R::METHOD, &parameters, timeout) {
            Ok(result) => result,
            Err(error) => return Err(self.request_error(error)),
        };

        serde_json::from_value(result).map_err(|error| {
            LspError::new(format!("cannot decode the {} result: {error}", R::METHOD))
        })
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
        let result = match self.request::<Initialize>(parameters, timeout) {
            Ok(result) => result,
            Err(error) => return Err(self.initialization_error(error)),
        };
        if let Err(error) = self.notify::<Initialized>(InitializedParams {}) {
            return Err(self.initialization_error(error));
        }
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
        let deadline = Instant::now() + timeout;

        loop {
            let message = match self.receive(method, deadline) {
                Ok(message) => message,
                Err(error) => return Err(self.transport_error(error)),
            };

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

    fn allocate_request_id(&mut self) -> u64 {
        self.request_id_counter += 1;
        self.request_id_counter
    }

    fn send(&mut self, message: &Value) -> Result<(), LspError> {
        if let Err(error) = write_message(&mut self.stdin, message) {
            return Err(self.transport_error(error));
        }

        Ok(())
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
            Ok(Ok(message)) => Ok(message),
            Ok(Err(error)) => Err(self.transport_error(error)),
            Err(RecvTimeoutError::Timeout) => Err(LspError::timeout(method)),
            Err(RecvTimeoutError::Disconnected) => {
                Err(self.transport_error(LspError::new("the language server reader stopped")))
            }
        }
    }

    fn answer_server_request(&mut self, message: &Value) -> Result<(), LspError> {
        self.send(&server_request_response(message))
    }

    /// Adds retained server standard error to a transport failure.
    fn transport_error(&mut self, error: LspError) -> LspError {
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

    /// Adds retained server standard error to an initialization failure.
    fn initialization_error(&mut self, error: LspError) -> LspError {
        self.request_error(error)
    }

    /// Adds retained server standard error to a request failure.
    fn request_error(&mut self, error: LspError) -> LspError {
        let deadline = Instant::now() + STDERR_CAPTURE_WAIT;

        while standard_error_is_empty(&self.standard_error) && Instant::now() < deadline {
            match self.process.try_wait() {
                Ok(Some(_)) => {
                    if let Some(thread) = self.standard_error_thread.take() {
                        let _ = thread.join();
                    }
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(1)),
                Err(_) => break,
            }
        }

        append_standard_error(error, &self.standard_error)
    }
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
const STDERR_CAPTURE_WAIT: Duration = Duration::from_millis(50);
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
    let suffix = format!("\n{STDERR_ERROR_HEADING}\n{standard_error}");

    if standard_error.is_empty() || error.to_string().ends_with(&suffix) {
        error
    } else {
        LspError::new(format!("{error}{suffix}"))
    }
}

fn standard_error_is_empty(capture: &Mutex<Vec<u8>>) -> bool {
    match capture.lock() {
        Ok(retained) => retained.is_empty(),
        Err(poisoned) => poisoned.into_inner().is_empty(),
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
/// Discards notifications. Sends other messages and read errors through the channel.
/// Stops after an error, a closed output, or a closed channel.
fn read_loop(mut reader: impl BufRead, sender: &Sender<Result<Value, LspError>>) {
    loop {
        match read_message(&mut reader) {
            Ok(Some(message)) => {
                // Notifications pass through. Each receive loop skips the
                // notifications it does not wait for.
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
/// # Errors
///
/// Returns an error if JSON serialization or a writer operation fails.
fn write_message(writer: &mut impl Write, message: &Value) -> Result<(), LspError> {
    let body = serde_json::to_vec(message)?;

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
// Path and Position Conversion
// -----------------------------------------------------------------------------
// The client relies on the LSP default UTF-16 position encoding. It advertises
// no other encoding.

/// Converts an absolute file path to an LSP file URI.
///
/// # Errors
///
/// Returns an error if the path is relative or file URL conversion fails.
pub fn file_uri(path: &Path) -> Result<Uri, LspError> {
    let url = Url::from_file_path(path).map_err(|()| {
        LspError::new(format!(
            "the LSP document path must be absolute: {}",
            path.display()
        ))
    })?;

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

/// Converts one zero-based UTF-16 LSP position into a UTF-8 byte offset.
///
/// # Errors
///
/// Returns an error if the line or character is outside the source text.
/// Returns an error if the character splits a UTF-16 surrogate pair.
pub(crate) fn position_to_byte_offset(
    source: &str,
    path: &str,
    position: Position,
) -> Result<u32, LspError> {
    let line_number = usize::try_from(position.line).map_err(|_| invalid_line(path, position))?;
    let line_start = source
        .split_inclusive('\n')
        .take(line_number)
        .map(str::len)
        .sum::<usize>();
    let line = source
        .get(line_start..)
        .and_then(|remainder| remainder.split('\n').next())
        .filter(|_| line_number < source.split('\n').count())
        .ok_or_else(|| invalid_line(path, position))?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut utf16_offset = 0_u32;

    for (byte_offset, character) in line.char_indices() {
        if utf16_offset == position.character {
            return absolute_offset(line_start, byte_offset, path);
        }

        let character_width = if character.len_utf16() == 2 { 2 } else { 1 };
        let next_offset = utf16_offset
            .checked_add(character_width)
            .ok_or_else(|| invalid_character(path, position))?;
        if position.character < next_offset {
            return Err(LspError::new(format!(
                "the LSP character {} splits a UTF-16 character in {path}",
                position.character
            )));
        }
        utf16_offset = next_offset;
    }

    if utf16_offset == position.character {
        return absolute_offset(line_start, line.len(), path);
    }

    Err(invalid_character(path, position))
}

/// # Errors
///
/// Returns an error if the combined offset does not fit in a source offset.
fn absolute_offset(line_start: usize, byte_offset: usize, path: &str) -> Result<u32, LspError> {
    line_start
        .checked_add(byte_offset)
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| LspError::new(format!("the source offset is too large in {path}")))
}

fn invalid_line(path: &str, position: Position) -> LspError {
    LspError::new(format!("the LSP line {} is outside {path}", position.line))
}

fn invalid_character(path: &str, position: Position) -> LspError {
    LspError::new(format!(
        "the LSP character {} is outside line {} in {path}",
        position.character, position.line
    ))
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
    };

    use lsp_types::Position;
    use serde_json::json;

    use super::{
        STDERR_CAPTURE_LIMIT, file_uri, position_to_byte_offset, read_message, read_standard_error,
        server_request_response, uri_path, write_message,
    };

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

    #[test]
    fn converts_utf16_positions_into_utf8_offsets() {
        let source = "alpha\né😀z\n";

        assert_eq!(
            position_to_byte_offset(source, "app.py", Position::new(1, 3))
                .expect("the position must be valid"),
            12
        );
        assert_eq!(
            position_to_byte_offset(source, "app.py", Position::new(2, 0))
                .expect("the final line must be valid"),
            14
        );
    }

    #[test]
    fn rejects_a_position_inside_a_utf16_surrogate_pair() {
        let result = position_to_byte_offset("😀", "app.py", Position::new(0, 1));

        assert!(result.is_err());
    }
}
