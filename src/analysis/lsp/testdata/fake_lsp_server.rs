//! Fake language server.
//!
//! The lsp tests compile this standalone program with `rustc` and start it as
//! a language server. The first argument selects the mode, for example
//! `serve`, `reject`, or `status-quiescent`. On `shutdown`, the program writes
//! a `shutdown-requested` marker next to its executable.
//!
//! Outside `script` mode, every document request answers `null`, except
//! that `semantic-reject-stderr` rejects document symbols and writes to
//! standard error.
//!
//! In `batch` mode, the program holds `test/echo` requests until it has three.
//! Then it sends a `workspace/configuration` request, checks the answer, and
//! echoes the three parameters in reverse order. `batch-error` answers the
//! second request with an error. `batch-slow` sends the echoes 300 ms apart.
//!
//! In `window-2` mode, the program echoes `test/echo` requests when no new
//! request arrives for 100 ms, and exits with an error when more than two
//! requests are unanswered.
//!
//! In `script` mode, the second argument names a script. Each script line holds
//! a method, a needle, and a result, separated by single spaces. A request gets
//! the result of the first line with its method whose needle matches: each
//! `&`-separated part of the needle is in the request, and `*` matches any
//! request. The client sends object keys in sorted order. `{root}` in a result
//! stands for the root URI. Every other request answers `null`. The program appends the method of each
//! request, `initialize` included, to a `requests.log` file next to its executable. When the
//! `FAKE_LSP_DELAY_MS` environment variable is set, each answer waits that
//! many milliseconds after the log line.

use std::{
    env, fs,
    io::{self, BufRead, Write},
    process,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

fn main() -> io::Result<()> {
    let mode = env::args().nth(1).unwrap_or_default();
    if mode == "fail" {
        eprintln!("invalid project configuration");
        process::exit(1);
    }

    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut root_uri = String::new();
    let mut echoes = Vec::new();
    let script = if mode == "script" {
        let path = env::args().nth(2).unwrap_or_default();
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    let messages = read_messages();

    while let Ok(message) = messages.recv() {
        let request_id = request_id(&message);

        if message.contains(r#""method":"initialize""#) {
            let request_id = required_id(request_id)?;
            if mode == "script" {
                log_request("initialize")?;
            }
            if mode == "reject" {
                eprintln!("rejected project configuration");
                write_error(&mut output, request_id, "invalid initialization")?;
                continue;
            }
            if mode == "undecodable-initialize" {
                eprintln!("broken capability report");
                write_result(&mut output, request_id, r#"{"capabilities":5}"#)?;
                continue;
            }
            if !message.contains(r#""initializationOptions":{"analysis":{"mode":"test"}}"#) {
                eprintln!("initialization options were not forwarded");
                process::exit(1);
            }
            root_uri = message
                .split_once(r#""rootUri":""#)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(uri, _)| uri.to_owned())
                .unwrap_or_default();
            write_result(
                &mut output,
                request_id,
                r#"{"capabilities":{"documentSymbolProvider":true,"callHierarchyProvider":true,"referencesProvider":true,"typeDefinitionProvider":true,"definitionProvider":true}}"#,
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
        } else if mode == "script"
            && request_id.is_some()
            && !message.contains(r#""method":"shutdown""#)
        {
            let method = message
                .split_once(r#""method":""#)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map_or("", |(method, _)| method);
            log_request(method)?;
            if let Some(delay) = env::var("FAKE_LSP_DELAY_MS")
                .ok()
                .and_then(|delay| delay.parse().ok())
            {
                thread::sleep(Duration::from_millis(delay));
            }
            let result = script
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, ' ');
                    Some((parts.next()?, parts.next()?, parts.next()?))
                })
                .find(|(each, needle, _)| {
                    *each == method
                        && (*needle == "*" || needle.split('&').all(|part| message.contains(part)))
                })
                .map_or_else(
                    || "null".to_owned(),
                    |(_, _, result)| result.replace("{root}", &root_uri),
                );
            write_result(&mut output, required_id(request_id)?, &result)?;
        } else if mode == "semantic-reject-stderr"
            && message.contains(r#""method":"textDocument/documentSymbol""#)
        {
            eprintln!("semantic request diagnostic");
            write_error(
                &mut output,
                required_id(request_id)?,
                "document symbol capture failed",
            )?;
        } else if mode == "window-2" && message.contains(r#""method":"test/echo""#) {
            echoes.push(echo(&message)?);
            while let Ok(next) = messages.recv_timeout(Duration::from_millis(100)) {
                echoes.push(echo(&next)?);
                if echoes.len() > 2 {
                    eprintln!("more than two requests are unanswered");
                    process::exit(1);
                }
            }
            for (id, parameters) in echoes.drain(..) {
                write_result(&mut output, &id, &parameters)?;
            }
        } else if message.contains(r#""method":"test/echo""#) {
            echoes.push(echo(&message)?);
            if echoes.len() < 3 {
                continue;
            }
            write_message(
                &mut output,
                r#"{"jsonrpc":"2.0","id":900,"method":"workspace/configuration","params":{"items":[{}]}}"#
                    .to_owned(),
            )?;
            let answer = messages.recv().unwrap_or_default();
            if !answer.contains(r#""id":900"#) || !answer.contains(r#""result":[{}]"#) {
                eprintln!("the client did not answer the configuration request");
                process::exit(1);
            }
            for (index, (id, parameters)) in echoes.drain(..).enumerate().rev() {
                // The first echo is the third request, so later echoes come 300 ms apart.
                if mode == "batch-slow" && index < 2 {
                    thread::sleep(Duration::from_millis(300));
                }
                if mode == "batch-error" && index == 1 {
                    write_error(&mut output, &id, "echo failed")?;
                } else {
                    write_result(&mut output, &id, &parameters)?;
                }
            }
        } else if message.contains(r#""method":"shutdown""#) {
            let executable = env::current_exe()?;
            let directory = executable
                .parent()
                .ok_or_else(|| io::Error::other("the fake server executable has no parent"))?;
            fs::write(directory.join("shutdown-requested"), "requested")?;
            if mode == "shutdown-error" {
                write_error(&mut output, required_id(request_id)?, "shutdown failed")?;
                continue;
            }
            write_result(&mut output, required_id(request_id)?, "null")?;
        } else if message.contains(r#""method":"exit""#) {
            break;
        } else if let Some(request_id) = request_id
            && (message.contains(r#""method":"textDocument/"#)
                || message.contains(r#""method":"callHierarchy/"#))
        {
            write_result(&mut output, request_id, "null")?;
        }
    }

    Ok(())
}

/// Appends one request method to the request log next to the executable.
fn log_request(method: &str) -> io::Result<()> {
    let executable = env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("the fake server executable has no parent"))?;
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("requests.log"))?;
    writeln!(log, "{method}")
}

/// Reads standard input on its own thread, so the program can wait for the
/// next message with a timeout.
fn read_messages() -> Receiver<String> {
    let (sender, messages) = mpsc::channel();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        while let Ok(Some(message)) = read_message(&mut input) {
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    messages
}

/// Returns the request ID and the raw parameters of one `test/echo` request.
fn echo(message: &str) -> io::Result<(String, String)> {
    let parameters = message
        .split_once(r#""params":"#)
        .and_then(|(_, rest)| rest.strip_suffix('}'))
        .unwrap_or("null")
        .to_owned();
    Ok((required_id(request_id(message))?.to_owned(), parameters))
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
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        }
    }

    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
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
