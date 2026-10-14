//! Fake language server.
//!
//! The lsp tests compile this standalone program with `rustc` and start it as
//! a language server. The first argument selects the mode, for example
//! `serve`, `reject`, or `status-quiescent`. On `shutdown`, the program writes
//! a `shutdown-requested` marker next to its executable.
//!
//! A definition request at line 0, character 16 (`run` in
//! `from lib import run`) answers `run` in `lib.py` of the workspace root. The
//! `definition-outside` mode answers a file outside the workspace instead.
//! Every other definition request answers `null`.
//!
//! In `module-call` mode, `run` in `lib.py` has a call hierarchy item, a
//! reference at line 2 of `app.py`, and an incoming call from the `app.py`
//! module at that reference. Every other call hierarchy request answers `[]`.
//!
//! In `batch` mode, the program holds `test/echo` requests until it has three.
//! Then it sends a `workspace/configuration` request, checks the answer, and
//! echoes the three parameters in reverse order. `batch-error` answers the
//! second request with an error.

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
    let mut root_uri = String::new();
    let mut echoes = Vec::new();

    while let Some(message) = read_message(&mut input)? {
        let request_id = request_id(&message);

        if message.contains(r#""method":"initialize""#) {
            let request_id = required_id(request_id)?;
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
        } else if message.contains(r#""method":"textDocument/prepareCallHierarchy""#) {
            let result = if mode == "module-call" && message.contains("/lib.py\"") {
                format!(
                    r#"[{{"name":"run","kind":12,"uri":"{root_uri}/lib.py","range":{RUN},"selectionRange":{RUN}}}]"#
                )
            } else {
                "[]".to_owned()
            };
            write_result(&mut output, required_id(request_id)?, &result)?;
        } else if message.contains(r#""method":"textDocument/references""#) {
            let result = if mode == "module-call" && message.contains("/lib.py\"") {
                format!(r#"[{{"uri":"{root_uri}/app.py","range":{CALL}}}]"#)
            } else {
                "[]".to_owned()
            };
            write_result(&mut output, required_id(request_id)?, &result)?;
        } else if message.contains(r#""method":"callHierarchy/incomingCalls""#) {
            let result = if mode == "module-call" {
                format!(
                    r#"[{{"from":{{"name":"(module) app.py","kind":2,"uri":"{root_uri}/app.py","range":{START},"selectionRange":{START}}},"fromRanges":[{CALL}]}}]"#
                )
            } else {
                "[]".to_owned()
            };
            write_result(&mut output, required_id(request_id)?, &result)?;
        } else if message.contains(r#""method":"test/echo""#) {
            let parameters = message
                .split_once(r#""params":"#)
                .and_then(|(_, rest)| rest.strip_suffix('}'))
                .unwrap_or("null")
                .to_owned();
            echoes.push((required_id(request_id)?.to_owned(), parameters));
            if echoes.len() < 3 {
                continue;
            }
            write_message(
                &mut output,
                r#"{"jsonrpc":"2.0","id":900,"method":"workspace/configuration","params":{"items":[{}]}}"#
                    .to_owned(),
            )?;
            let answer = read_message(&mut input)?.unwrap_or_default();
            if !answer.contains(r#""id":900"#) || !answer.contains(r#""result":[{}]"#) {
                eprintln!("the client did not answer the configuration request");
                process::exit(1);
            }
            for (index, (id, parameters)) in echoes.drain(..).enumerate().rev() {
                if mode == "batch-error" && index == 1 {
                    write_error(&mut output, &id, "echo failed")?;
                } else {
                    write_result(&mut output, &id, &parameters)?;
                }
            }
        } else if message.contains(r#""method":"callHierarchy/outgoingCalls""#) {
            write_result(&mut output, required_id(request_id)?, "[]")?;
        } else if message.contains(r#""method":"textDocument/typeDefinition""#) {
            write_result(&mut output, required_id(request_id)?, "null")?;
        } else if message.contains(r#""method":"textDocument/definition""#) {
            if !message.contains(r#""line":0"#) || !message.contains(r#""character":16"#) {
                write_result(&mut output, required_id(request_id)?, "null")?;
                continue;
            }
            let uri = if mode == "definition-outside" {
                "file:///outside/lib.py".to_owned()
            } else {
                format!("{root_uri}/lib.py")
            };
            write_result(
                &mut output,
                required_id(request_id)?,
                &format!(
                    r#"{{"uri":{uri:?},"range":{{"start":{{"line":0,"character":4}},"end":{{"line":0,"character":7}}}}}}"#
                ),
            )?;
        } else if message.contains(r#""method":"shutdown""#) {
            let executable = env::current_exe()?;
            let directory = executable
                .parent()
                .ok_or_else(|| io::Error::other("the fake server executable has no parent"))?;
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

/// The `run` name in `def run():`.
const RUN: &str = r#"{"start":{"line":0,"character":4},"end":{"line":0,"character":7}}"#;
/// The `run` name in a module-level `run()` call on line 2.
const CALL: &str = r#"{"start":{"line":2,"character":0},"end":{"line":2,"character":3}}"#;
/// The start of a file.
const START: &str = r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}"#;

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
