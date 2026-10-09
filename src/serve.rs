//! Viewer server.
//!
//! This module serves the Lift Deck on the loopback address. The viewer page,
//! its script, its stylesheet, and its fonts come from the binary. Store files
//! come from one store directory, and only when its `index.json` lists them.
//! The server never writes. A store directory and a port are the inputs. HTTP
//! responses are the output.
//!
//! # Sections
//!
//! `Server` binds the address and answers requests.
//! `Requests` checks the headers and the method of one request.
//! `Routes` maps a request path onto an embedded asset or a store file.
//! `Index allowlist` decides which store files the viewer can read.
//! `Safe reads` reads one store file without following a link.

use std::{
    borrow::Cow,
    fs, io,
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    flow::models::is_flow_id,
    store::{graph::check_graph_name, reject_symlink},
};

// -----------------------------------------------------------------------------
// Server
// -----------------------------------------------------------------------------

const PAGE: &[u8] = include_bytes!("../viewer/index.html");
const SCRIPT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/viewer/app.js"));
const STYLE: &[u8] = include_bytes!("../viewer/styles/deck.css");
const FONTS: [(&str, &[u8]); 2] = [
    (
        "jetbrains-mono-latin-400.woff2",
        include_bytes!("../viewer/fonts/jetbrains-mono-latin-400.woff2"),
    ),
    (
        "jetbrains-mono-latin-700.woff2",
        include_bytes!("../viewer/fonts/jetbrains-mono-latin-700.woff2"),
    ),
];

const RESPONSE_HEADERS: [(&str, &str); 4] = [
    ("Cache-Control", "no-store"),
    ("X-Content-Type-Options", "nosniff"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("Referrer-Policy", "no-referrer"),
];

/// One viewer server bound to `127.0.0.1`.
pub struct ViewerServer {
    http: tiny_http::Server,
    store: PathBuf,
    host: String,
}

impl ViewerServer {
    /// Binds the loopback address for one store. Port 0 selects a free port.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is missing, is not a directory, or is a
    /// symbolic link, or if the port is not free.
    pub fn bind(store: &Path, port: u16) -> io::Result<Self> {
        reject_symlink(store)?;
        if !fs::metadata(store)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("cannot open the store {}: {error}", store.display()),
                )
            })?
            .is_dir()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the store {} is not a directory", store.display()),
            ));
        }
        let http = tiny_http::Server::http(("127.0.0.1", port)).map_err(|error| {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("cannot listen on 127.0.0.1:{port}: {error}"),
            )
        })?;
        let port = http
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("the server has no IP address"))?
            .port();
        Ok(Self {
            http,
            store: store.to_path_buf(),
            host: format!("127.0.0.1:{port}"),
        })
    }

    /// Returns the viewer page URL.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/viewer/", self.host)
    }

    /// Answers requests until the listener closes.
    pub fn run(&self) {
        for request in self.http.incoming_requests() {
            let headers: Vec<(String, String)> = request
                .headers()
                .iter()
                .map(|header| {
                    (
                        header.field.as_str().as_str().to_owned(),
                        header.value.as_str().to_owned(),
                    )
                })
                .collect();
            let reply = answer(
                &self.store,
                &self.host,
                request.method().as_str(),
                request.url(),
                &headers,
            );
            if let Err(error) = request.respond(reply.into_response()) {
                eprintln!("warning: cannot send a response: {error}");
            }
        }
    }
}

/// One response before it goes on the wire.
#[derive(Debug)]
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Cow<'static, [u8]>,
    location: Option<&'static str>,
}

impl Reply {
    fn asset(content_type: &'static str, body: &'static [u8]) -> Self {
        Self {
            status: 200,
            content_type,
            body: Cow::Borrowed(body),
            location: None,
        }
    }

    fn failure(status: u16, message: &str) -> Self {
        Self {
            status,
            content_type: JSON,
            body: Cow::Owned(
                serde_json::json!({ "error": message })
                    .to_string()
                    .into_bytes(),
            ),
            location: None,
        }
    }

    /// Builds the HTTP response. A HEAD response keeps the headers and drops the body.
    fn into_response(self) -> tiny_http::Response<io::Cursor<Vec<u8>>> {
        let mut headers = vec![("Content-Type", self.content_type)];
        headers.extend(RESPONSE_HEADERS);
        headers.extend(self.location.map(|location| ("Location", location)));
        headers
            .into_iter()
            .filter_map(|(name, value)| tiny_http::Header::from_bytes(name, value).ok())
            .fold(
                tiny_http::Response::from_data(self.body.into_owned())
                    .with_status_code(self.status)
                    .with_chunked_threshold(usize::MAX),
                tiny_http::Response::with_header,
            )
    }
}

// -----------------------------------------------------------------------------
// Requests
// -----------------------------------------------------------------------------

/// Answers one request.
///
/// A request needs exactly one `Host` header that names this server, at most
/// one `Origin` header that names this server, and the GET or HEAD method.
fn answer(
    store: &Path,
    host: &str,
    method: &str,
    url: &str,
    headers: &[(String, String)],
) -> Reply {
    let values = |name: &str| -> Vec<&str> {
        headers
            .iter()
            .filter(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    };
    let hosts = values("host");
    let origins = values("origin");
    let origin = format!("http://{host}");
    if hosts.len() > 1 || origins.len() > 1 {
        return Reply::failure(403, "Duplicate security header.");
    }
    if hosts.first() != Some(&host) {
        return Reply::failure(403, "Unexpected Host header.");
    }
    if origins.first().is_some_and(|value| *value != origin) {
        return Reply::failure(403, "Cross-origin requests are not allowed.");
    }
    if method != "GET" && method != "HEAD" {
        return Reply::failure(405, "Use GET or HEAD.");
    }
    match route_parts(url).and_then(|parts| route(store, &parts)) {
        Ok(reply) | Err(reply) => reply,
    }
}

// -----------------------------------------------------------------------------
// Routes
// -----------------------------------------------------------------------------

const JSON: &str = "application/json; charset=utf-8";

/// Returns the embedded asset or the store file of an allowed path. Any other path is a 404.
fn route(store: &Path, parts: &[String]) -> Result<Reply, Reply> {
    let words: Vec<&str> = parts.iter().map(String::as_str).collect();
    match words.as_slice() {
        // Send the root to the viewer page.
        [""] => Ok(Reply {
            status: 302,
            content_type: JSON,
            body: Cow::Borrowed(b"{}"),
            location: Some("/viewer/"),
        }),
        // Serve the viewer page.
        ["viewer", "" | "index.html"] => Ok(Reply::asset("text/html; charset=utf-8", PAGE)),
        // Serve the viewer script.
        ["viewer", "dist", "app.js"] => Ok(Reply::asset("text/javascript; charset=utf-8", SCRIPT)),
        // Serve the viewer stylesheet.
        ["viewer", "dist", "app.css"] => Ok(Reply::asset("text/css; charset=utf-8", STYLE)),
        // Serve one embedded font.
        ["viewer", "fonts", name] => FONTS
            .iter()
            .find(|(font, _)| font == name)
            .map(|(_, bytes)| Reply::asset("font/woff2", bytes))
            .ok_or_else(not_found),
        // Serve the index and the store files that it lists.
        ["artifacts", rest @ ..] if !rest.is_empty() => {
            let file = rest.join("/");
            if file != "index.json" && !listed(&read_index(store)?, &file) {
                return Err(not_found());
            }
            let bytes = read_safe(store, rest)?;
            Ok(Reply {
                status: 200,
                content_type: JSON,
                body: Cow::Owned(bytes),
                location: None,
            })
        }
        // Reject every other path.
        _ => Err(not_found()),
    }
}

fn not_found() -> Reply {
    Reply::failure(404, "File not found.")
}

/// Splits a request path into decoded components.
///
/// A path must start with one `/`. Dot components, separators, control
/// characters, and invalid percent escapes are a 400.
fn route_parts(url: &str) -> Result<Vec<String>, Reply> {
    let invalid = || Reply::failure(400, "Invalid request path.");
    let path = url.split('?').next().unwrap_or_default();
    let path = path
        .strip_prefix('/')
        .filter(|rest| !rest.starts_with('/'))
        .ok_or_else(invalid)?;
    path.split('/')
        .map(|part| {
            let decoded = percent_decode(part).ok_or_else(invalid)?;
            if decoded == "."
                || decoded == ".."
                || decoded
                    .chars()
                    .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
            {
                return Err(invalid());
            }
            Ok(decoded)
        })
        .collect()
}

/// Decodes `%XX` escapes. An incomplete escape or invalid UTF-8 returns `None`.
fn percent_decode(part: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(part.len());
    let mut input = part.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = char::from(input.next()?).to_digit(16)?;
            let low = char::from(input.next()?).to_digit(16)?;
            bytes.push(u8::try_from(high * 16 + low).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

// -----------------------------------------------------------------------------
// Index allowlist
// -----------------------------------------------------------------------------

fn read_index(store: &Path) -> Result<Value, Reply> {
    let bytes = read_safe(store, &["index.json"])?;
    serde_json::from_slice(&bytes).map_err(|_| Reply::failure(500, "Cannot read the store index."))
}

/// True when the index lists the file: a graph file, or a schema 3 flow generation under `flows/`.
fn listed(index: &Value, file: &str) -> bool {
    let entries = index["artifacts"].as_array().map_or(&[][..], Vec::as_slice);
    entries.iter().any(|entry| {
        entry["file"]
            .as_str()
            .is_some_and(|name| graph_name(name) && name == file)
            || entry["flows"].as_array().is_some_and(|flows| {
                flows.iter().any(|flow| {
                    flow["schemaVersion"] == 3
                        && flow["file"].as_str().is_some_and(|path| {
                            generation_file(path) && file.strip_prefix("flows/") == Some(path)
                        })
                })
            })
    })
}

/// Accepts a graph file name: one store entry that ends in `.json`, does not start with a dot, and is not the index.
fn graph_name(name: &str) -> bool {
    !name.starts_with('.') && check_graph_name(name).is_ok()
}

/// Accepts a generation path: `<flow ID>/generations/<ordinal>-<digest12>.json`.
fn generation_file(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let [id, "generations", name] = parts.as_slice() else {
        return false;
    };
    let Some((ordinal, digest)) = name
        .strip_suffix(".json")
        .and_then(|stem| stem.split_once('-'))
    else {
        return false;
    };
    is_flow_id(id)
        && (1..=9).contains(&ordinal.len())
        && !ordinal.starts_with('0')
        && ordinal.bytes().all(|byte| byte.is_ascii_digit())
        && digest.len() == 12
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// -----------------------------------------------------------------------------
// Safe reads
// -----------------------------------------------------------------------------

/// Reads one regular store file. No component of the path can be a symbolic link.
fn read_safe(store: &Path, parts: &[&str]) -> Result<Vec<u8>, Reply> {
    let mut path = store.to_path_buf();
    check_link(&path)?;
    for part in parts {
        path.push(part);
        check_link(&path)?;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(&path).map_err(|error| read_failure(&error))?;
    if !file
        .metadata()
        .map_err(|error| read_failure(&error))?
        .is_file()
    {
        return Err(Reply::failure(403, "The path must be a regular file."));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| read_failure(&error))?;
    Ok(bytes)
}

fn check_link(path: &Path) -> Result<(), Reply> {
    match reject_symlink(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
            Err(Reply::failure(403, "Symbolic links are not allowed."))
        }
        Err(error) => Err(read_failure(&error)),
    }
}

/// Maps a read error to a reply. A link that appears after the link check fails with `ELOOP`.
fn read_failure(error: &io::Error) -> Reply {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return Reply::failure(403, "Symbolic links are not allowed.");
    }
    if error.kind() == io::ErrorKind::NotFound {
        not_found()
    } else {
        Reply::failure(500, "Cannot access the viewer store.")
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpStream,
        path::Path,
    };

    use super::{PAGE, Reply, ViewerServer, answer};

    const GENERATION: &str = "demo-flow/generations/1-0123456789ab.json";
    const HOST: &str = "127.0.0.1:9";

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("store");
        let store = root.path();
        fs::create_dir_all(store.join("flows/demo-flow/generations")).expect("folders");
        let graph = r#"{"schemaVersion":1,"graph":{"nodes":[],"edges":[]}}"#;
        let index = serde_json::json!({"schemaVersion": 3, "defaultArtifact": "graph.json", "artifacts": [
            {"file": "graph.json", "label": "Graph", "digest": "stale", "flows": [
                {"name": "demo-flow", "file": GENERATION, "schemaVersion": 3},
                {"name": "loose", "file": "demo-flow/current.json", "schemaVersion": 3},
                {"name": "older", "file": "demo-flow/generations/2-0123456789ab.json"},
                {"name": "legacy", "file": "legacy.json"}
            ]},
            {"file": "same.json", "flows": []},
            {"file": ".hidden.json", "flows": []}
        ]});
        for (file, text) in [
            ("graph.json", graph.to_owned()),
            ("same.json", graph.to_owned()),
            (".hidden.json", graph.to_owned()),
            ("index.json", index.to_string()),
            ("secret.json", r#"{"secret":true}"#.to_owned()),
            (
                &format!("flows/{GENERATION}"),
                r#"{"schemaVersion":3}"#.to_owned(),
            ),
            (
                "flows/demo-flow/generations/2-0123456789ab.json",
                r#"{"schemaVersion":3}"#.to_owned(),
            ),
            ("flows/demo-flow/current.json", "{}".to_owned()),
            ("flows/legacy.json", r#"{"schemaVersion":2}"#.to_owned()),
        ] {
            fs::write(store.join(file), text).expect("write fixture");
        }
        root
    }

    fn get(store: &Path, url: &str) -> Reply {
        answer(
            store,
            HOST,
            "GET",
            url,
            &[("Host".to_owned(), HOST.to_owned())],
        )
    }

    fn status(store: &Path, url: &str) -> u16 {
        get(store, url).status
    }

    fn assert_error(reply: &Reply, status: u16) {
        assert_eq!(reply.status, status);
        let body: serde_json::Value = serde_json::from_slice(&reply.body).expect("JSON error");
        assert!(body["error"].is_string());
        assert_eq!(body.as_object().map(serde_json::Map::len), Some(1));
    }

    #[test]
    fn serves_the_embedded_page_script_stylesheet_and_fonts_only() {
        let root = fixture();
        let store = root.path();
        let page = get(store, "/viewer/");
        assert_eq!(
            (page.status, page.content_type),
            (200, "text/html; charset=utf-8")
        );
        assert_eq!(&*page.body, PAGE);
        assert_eq!(&*get(store, "/viewer/index.html?artifact=a").body, PAGE);
        assert!(
            get(store, "/viewer/dist/app.js")
                .content_type
                .contains("javascript")
        );
        assert!(!get(store, "/viewer/dist/app.js").body.is_empty());
        assert!(
            get(store, "/viewer/dist/app.css")
                .content_type
                .starts_with("text/css")
        );
        let font = get(store, "/viewer/fonts/jetbrains-mono-latin-400.woff2");
        assert_eq!((font.status, font.content_type), (200, "font/woff2"));
        let root_page = get(store, "/");
        assert_eq!(
            (root_page.status, root_page.location),
            (302, Some("/viewer/"))
        );
        for path in [
            "/viewer",
            "/viewer/src/main.ts",
            "/viewer/styles/deck.css",
            "/viewer/fonts/OFL.txt",
            "/viewer/dist/other.js",
            "/viewer/deck.html",
            "/AGENTS.md",
            "/api/session",
        ] {
            assert_error(&get(store, path), 404);
        }
    }

    #[test]
    fn serves_the_index_listed_graphs_and_listed_schema_3_generations_only() {
        let root = fixture();
        let store = root.path();
        assert_eq!(status(store, "/artifacts/index.json"), 200);
        assert_eq!(status(store, "/artifacts/graph.json"), 200);
        assert_eq!(status(store, "/artifacts/same.json"), 200);
        assert_eq!(
            status(store, &format!("/artifacts/flows/{GENERATION}")),
            200
        );
        for path in [
            "/artifacts/secret.json",
            "/artifacts/.hidden.json",
            "/artifacts/flows/legacy.json",
            "/artifacts/flows/demo-flow/current.json",
            "/artifacts/flows/demo-flow/generations/2-0123456789ab.json",
            "/artifacts/notes/a.json",
            "/artifacts/.store.lock",
            "/artifacts/",
        ] {
            assert_error(&get(store, path), 404);
        }
    }

    #[test]
    fn requires_the_exact_host_the_same_origin_and_a_read_method() {
        let root = fixture();
        let store = root.path();
        let host = ("Host".to_owned(), HOST.to_owned());
        for value in ["evil.example", "localhost:9", "127.0.0.1:1"] {
            let reply = answer(
                store,
                HOST,
                "GET",
                "/viewer/",
                &[("Host".to_owned(), value.to_owned())],
            );
            assert_error(&reply, 403);
        }
        assert_error(&answer(store, HOST, "GET", "/viewer/", &[]), 403);
        assert_error(
            &answer(
                store,
                HOST,
                "GET",
                "/viewer/",
                &[host.clone(), host.clone()],
            ),
            403,
        );
        let evil = ("Origin".to_owned(), "http://evil.example".to_owned());
        assert_error(
            &answer(store, HOST, "GET", "/viewer/", &[host.clone(), evil]),
            403,
        );
        let same = ("origin".to_owned(), format!("http://{HOST}"));
        assert_eq!(
            answer(
                store,
                HOST,
                "GET",
                "/viewer/",
                &[host.clone(), same.clone()]
            )
            .status,
            200
        );
        assert_error(
            &answer(
                store,
                HOST,
                "GET",
                "/viewer/",
                &[host.clone(), same.clone(), same],
            ),
            403,
        );
        for method in ["PUT", "POST", "DELETE"] {
            assert_error(
                &answer(
                    store,
                    HOST,
                    method,
                    "/artifacts/graph.json",
                    std::slice::from_ref(&host),
                ),
                405,
            );
        }
    }

    #[test]
    fn rejects_traversal_encoded_separators_and_symbolic_links() {
        let root = fixture();
        let store = root.path();
        for path in [
            "/artifacts/../secret.json",
            "/viewer/%2e%2e/AGENTS.md",
            "/artifacts/%2fetc%2fpasswd",
            "/artifacts/flows%5cdemo.json",
            "/artifacts/%00",
            "/viewer/%zz",
            "//artifacts/index.json",
            "artifacts/index.json",
        ] {
            assert_error(&get(store, path), 400);
        }
        #[cfg(unix)]
        {
            fs::remove_file(store.join("graph.json")).expect("remove graph");
            std::os::unix::fs::symlink(store.join("secret.json"), store.join("graph.json"))
                .expect("link graph");
            assert_error(&get(store, "/artifacts/graph.json"), 403);
            fs::remove_dir_all(store.join("flows")).expect("remove flows");
            let outside = tempfile::tempdir().expect("outside");
            std::os::unix::fs::symlink(outside.path(), store.join("flows")).expect("link flows");
            assert_error(&get(store, &format!("/artifacts/flows/{GENERATION}")), 403);
        }
    }

    #[test]
    fn answers_over_http_with_security_headers_and_headless_head_responses() {
        let root = fixture();
        let server = ViewerServer::bind(root.path(), 0).expect("bind");
        let url = server.url();
        let host = url
            .trim_start_matches("http://")
            .trim_end_matches("/viewer/")
            .to_owned();
        std::thread::spawn(move || server.run());
        let request = |text: String| {
            let mut stream = TcpStream::connect(&host).expect("connect");
            stream.write_all(text.as_bytes()).expect("send");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read");
            response
        };

        let page = request(format!(
            "GET /viewer/ HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        ));
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("Cache-Control: no-store"));
        assert!(page.contains("X-Content-Type-Options: nosniff"));
        assert!(page.contains(&format!("Content-Length: {}", PAGE.len())));
        let head = request(format!(
            "HEAD /artifacts/secret.json HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        ));
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        assert!(
            head.ends_with("\r\n\r\n"),
            "a HEAD response carries no body: {head}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_missing_or_linked_store() {
        let root = fixture();
        assert!(ViewerServer::bind(&root.path().join("missing"), 0).is_err());
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(root.path(), &linked).expect("link store");
        assert!(ViewerServer::bind(&linked, 0).is_err());
    }
}
