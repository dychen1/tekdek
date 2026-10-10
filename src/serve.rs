//! Viewer server.
//!
//! This module serves the Lift Deck on the loopback address. The viewer page,
//! its script, its stylesheet, and its fonts come from the binary. Store files
//! come from one store directory, and only when its catalog lists them. The
//! `/source` route also reads the source files of the step functions of a flow.
//! It reads them inside the project folder that holds the store. The server
//! builds the catalog from the store for each request and never writes. A store
//! directory and a port are the inputs. HTTP responses are the output.
//!
//! # Sections
//!
//! `Server` binds the address and answers requests.
//! `Requests` checks the headers and the method of one request.
//! `Routes` maps a request path onto an embedded asset or a store file.
//! `Catalog allowlist` decides which store files the viewer can read.
//! `Source route` serves the function source of one flow.
//! `Safe reads` reads one store file without following a link.

use std::{
    borrow::Cow,
    fs, io,
    io::Read,
    path::{Path, PathBuf},
};

use crate::{
    catalog::{build_catalog, readable_files},
    flow::{
        lineage::read_current,
        models::{FlowId, is_flow_id},
    },
    source_view::{SourceDocument, flow_sources},
    store::{
        graph::{check_graph_name, read_graph},
        reject_symlink,
    },
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

    fn json(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: JSON,
            body: Cow::Owned(body),
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
    let query = url.split_once('?').map(|(_, query)| query);
    match route_parts(url).and_then(|parts| route(store, &parts, query)) {
        Ok(reply) | Err(reply) => reply,
    }
}

// -----------------------------------------------------------------------------
// Routes
// -----------------------------------------------------------------------------

const JSON: &str = "application/json; charset=utf-8";

/// Returns the embedded asset or the store file of an allowed path. Any other path is a 404.
fn route(store: &Path, parts: &[String], query: Option<&str>) -> Result<Reply, Reply> {
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
        // Serve the catalog that the server builds from the store.
        ["artifacts", "index.json"] => Ok(Reply::json(catalog_document(store)?)),
        // Serve the store files that the catalog lists.
        ["artifacts", rest @ ..] if !rest.is_empty() => {
            let file = rest.join("/");
            if !listed(&catalog_files(store)?, &file) {
                return Err(not_found());
            }
            Ok(Reply::json(read_safe(store, rest)?))
        }
        // Serve the source of every step function of one flow.
        ["source"] => Ok(Reply::json(source_document(store, query)?)),
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
// Catalog allowlist
// -----------------------------------------------------------------------------

/// Returns the catalog JSON. A store without a graph file is a 404.
fn catalog_document(store: &Path) -> Result<Vec<u8>, Reply> {
    check_link(store)?;
    let catalog = build_catalog(store).map_err(|error| catalog_failure(&error))?;
    serde_json::to_vec(&catalog).map_err(|_| Reply::failure(500, "Cannot build the store catalog."))
}

fn catalog_files(store: &Path) -> Result<Vec<String>, Reply> {
    check_link(store)?;
    readable_files(store).map_err(|error| catalog_failure(&error))
}

fn catalog_failure(error: &io::Error) -> Reply {
    if error.kind() == io::ErrorKind::InvalidInput {
        not_found()
    } else {
        Reply::failure(500, "Cannot build the store catalog.")
    }
}

/// True when the catalog lists the file: a graph file, or a current flow generation under `flows/`.
fn listed(files: &[String], file: &str) -> bool {
    let named = match file.strip_prefix("flows/") {
        Some(path) => generation_file(path),
        None => graph_name(file),
    };
    named && files.iter().any(|listed| listed == file)
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
// Source route
// -----------------------------------------------------------------------------

/// The largest project file that the source route reads.
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

/// Returns the source JSON of every step function of one flow.
///
/// The flow must be in the catalog under the named graph. File paths come
/// from the graph, never from the query, and stay inside the project folder
/// that holds the store.
fn source_document(store: &Path, query: Option<&str>) -> Result<Vec<u8>, Reply> {
    let (artifact, flow) = source_query(query)?;
    check_link(store)?;
    let catalog = build_catalog(store).map_err(|error| catalog_failure(&error))?;
    if catalog.graph_of_flow(&flow) != Some(artifact.as_str()) {
        return Err(not_found());
    }
    let (graph, _) = read_graph(store, &artifact).map_err(|error| read_failure(&error))?;
    let flow_id = FlowId::try_from(flow.as_str()).map_err(|_| not_found())?;
    let current = read_current(store, &flow_id)
        .map_err(|_| Reply::failure(500, "Cannot read the flow."))?
        .ok_or_else(not_found)?;
    let store = fs::canonicalize(store).map_err(|error| read_failure(&error))?;
    let root = store
        .parent()
        .ok_or_else(|| Reply::failure(500, "The store has no project folder."))?;
    let functions = flow_sources(&graph, &current.artifact, |path| read_project(root, path));
    serde_json::to_vec(&SourceDocument {
        artifact,
        flow,
        functions,
    })
    .map_err(|_| Reply::failure(500, "Cannot build the source document."))
}

/// Reads `artifact` and `flow`, each exactly once. Any other parameter is a 400.
fn source_query(query: Option<&str>) -> Result<(String, String), Reply> {
    let invalid = || Reply::failure(400, "Use /source?artifact=<graph file>&flow=<flow name>.");
    let (mut artifact, mut flow) = (None, None);
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
    {
        let (name, value) = pair.split_once('=').ok_or_else(invalid)?;
        let value = percent_decode(value).ok_or_else(invalid)?;
        let slot = match name {
            "artifact" => &mut artifact,
            "flow" => &mut flow,
            _ => return Err(invalid()),
        };
        if slot.replace(value).is_some() {
            return Err(invalid());
        }
    }
    let (Some(artifact), Some(flow)) = (artifact, flow) else {
        return Err(invalid());
    };
    if !graph_name(&artifact) || !is_flow_id(&flow) {
        return Err(not_found());
    }
    Ok((artifact, flow))
}

/// Reads one project file as UTF-8 text, or returns why it cannot.
///
/// No component of the path can be a symbolic link.
fn read_project(root: &Path, relative: &str) -> Result<String, String> {
    let cannot = |what: &str| format!("the file cannot be {what}");
    if !project_path(relative) {
        return Err("the path leaves the project".to_owned());
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        match fs::symlink_metadata(&path) {
            // Refuse a symbolic link at any component.
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("the path holds a symbolic link".to_owned());
            }
            // Keep walking when the component exists.
            Ok(_) => {}
            // Report a component that does not exist.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err("the file is missing".to_owned());
            }
            // Report any other failure.
            Err(_) => return Err(cannot("opened")),
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).map_err(|_| cannot("opened"))?;
    let metadata = file.metadata().map_err(|_| cannot("opened"))?;
    if !metadata.is_file() {
        return Err("the path is not a regular file".to_owned());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err("the file is larger than 2 MiB".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| cannot("read"))?;
    // The file can grow between the metadata call and the read.
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err("the file is larger than 2 MiB".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())
}

/// Accepts a relative path with no empty, `.`, or `..` component, no backslash, and no colon.
fn project_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
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

    use super::{
        JSON, MAX_SOURCE_BYTES, PAGE, Reply, ViewerServer, answer, project_path, read_project,
    };
    use crate::flow::fixtures::{self, frozen, import, output};

    const HOST: &str = "127.0.0.1:9";

    /// A store with one frozen flow, its graph, and files that the catalog does not list.
    ///
    /// Returns the store and the path of the current generation under `flows/`.
    fn fixture() -> (tempfile::TempDir, String) {
        let root = fixtures::store();
        let store = root.path();
        let generation = frozen(import(store, "demo-flow", &output())).file;
        let graph = r#"{"schemaVersion":1,"graph":{"nodes":[],"edges":[]}}"#;
        fs::create_dir_all(store.join("notes")).expect("folders");
        for (file, text) in [
            ("same.json", graph),
            (".hidden.json", graph),
            ("index.json", "<<<<<<< HEAD"),
            ("secret.txt", "secret"),
            ("notes/a.json", "{}"),
            (
                "flows/demo-flow/generations/2-0123456789ab.json",
                r#"{"schemaVersion":3}"#,
            ),
            ("flows/legacy.json", r#"{"schemaVersion":2}"#),
        ] {
            fs::write(store.join(file), text).expect("write fixture");
        }
        (root, generation)
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
        let (root, _) = fixture();
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
    fn serves_the_built_catalog_its_graphs_and_current_generations_only() {
        let (root, generation) = fixture();
        let store = root.path();
        let catalog = get(store, "/artifacts/index.json");
        assert_eq!(catalog.status, 200);
        let catalog: serde_json::Value = serde_json::from_slice(&catalog.body).expect("catalog");
        assert_eq!(catalog["defaultArtifact"], "graph.json");
        assert_eq!(catalog["artifacts"][0]["flows"][0]["file"], generation);
        assert_eq!(catalog["artifacts"].as_array().map(Vec::len), Some(2));
        assert_eq!(status(store, "/artifacts/graph.json"), 200);
        assert_eq!(status(store, "/artifacts/same.json"), 200);
        assert_eq!(
            status(store, &format!("/artifacts/flows/{generation}")),
            200
        );
        for path in [
            "/artifacts/secret.txt",
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
        let empty = tempfile::tempdir().expect("empty store");
        assert_error(&get(empty.path(), "/artifacts/index.json"), 404);
    }

    #[test]
    fn requires_the_exact_host_the_same_origin_and_a_read_method() {
        let (root, _) = fixture();
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
        let (root, generation) = fixture();
        let store = root.path();
        for path in [
            "/artifacts/../secret.txt",
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
            std::os::unix::fs::symlink(store.join("secret.txt"), store.join("graph.json"))
                .expect("link graph");
            assert_error(&get(store, "/artifacts/graph.json"), 404);
            let flows = store.join("flows");
            let moved = tempfile::tempdir().expect("outside");
            fs::rename(&flows, moved.path().join("flows")).expect("move flows");
            std::os::unix::fs::symlink(moved.path().join("flows"), &flows).expect("link flows");
            assert_error(&get(store, &format!("/artifacts/flows/{generation}")), 404);
        }
    }

    #[test]
    fn answers_over_http_with_security_headers_and_headless_head_responses() {
        let (root, _) = fixture();
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
            "HEAD /artifacts/secret.txt HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
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
        let (root, _) = fixture();
        assert!(ViewerServer::bind(&root.path().join("missing"), 0).is_err());
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(root.path(), &linked).expect("link store");
        assert!(ViewerServer::bind(&linked, 0).is_err());
    }

    /// Sets the declaration, scope, body, and content hash of a node from its source text.
    fn set_ranges(node: &mut crate::store::graph::ArtifactNode, text: &str) {
        use crate::{analysis::source::ContentHash, store::graph::ArtifactRange};
        let offset = |index: usize| u32::try_from(index).expect("offset");
        let name = text.find(&node.name).expect("name");
        let open = text.find('{').expect("body");
        let close = text.rfind('}').expect("body end") + 1;
        node.declarations = vec![ArtifactRange {
            start: offset(name),
            end: offset(name + node.name.len()),
        }];
        node.scope = Some(ArtifactRange {
            start: offset(text.find("fn ").expect("fn")),
            end: offset(close),
        });
        node.body = Some(ArtifactRange {
            start: offset(open),
            end: offset(close),
        });
        node.content_hash = Some(ContentHash::of(&text[name..close]).to_string());
    }

    /// A project folder with a store, one frozen flow, a graph with source ranges, and source files.
    ///
    /// Step functions: `main` and `parse_args` are readable, `read_input` is not in the graph,
    /// `write_result` leaves the project, and `lock_store` changed after the index.
    fn source_fixture() -> tempfile::TempDir {
        let project = tempfile::tempdir().expect("project");
        let store = project.path().join(".tekdek");
        fs::create_dir(&store).expect("store");
        let mut graph = fixtures::write_graph(&store, "src/extra.rs::callable:helper");
        frozen(import(&store, "demo-flow", &output()));
        let main = "fn main() {\n    parse_args();\n}\n";
        let parse = "/// Parses the flags.\nfn parse_args() -> Args {\n    Args\n}\n";
        let lock = "fn lock_store() {}\n";
        fs::create_dir(project.path().join("src")).expect("src");
        fs::write(project.path().join("src/main.rs"), main).expect("main");
        fs::write(project.path().join("src/parse.rs"), parse).expect("parse");
        fs::write(
            project.path().join("src/store.rs"),
            "fn lock_store() { changed(); }\n",
        )
        .expect("store");
        let nodes = &mut graph.graph.nodes;
        set_ranges(&mut nodes[0], main);
        set_ranges(&mut nodes[1], parse);
        nodes[2].key = "src/parse.rs::callable:renamed".to_owned();
        nodes[3].path = "../outside.rs".to_owned();
        set_ranges(&mut nodes[4], lock);
        graph.write(&store.join("graph.json")).expect("graph");
        project
    }

    #[test]
    fn serves_the_source_of_each_step_function_once_in_step_order() {
        let project = source_fixture();
        let reply = get(
            &project.path().join(".tekdek"),
            "/source?artifact=graph.json&flow=demo-flow",
        );
        assert_eq!((reply.status, reply.content_type), (200, JSON));
        let body: serde_json::Value = serde_json::from_slice(&reply.body).expect("source JSON");
        assert_eq!(
            (body["artifact"].as_str(), body["flow"].as_str()),
            (Some("graph.json"), Some("demo-flow"))
        );
        let functions = body["functions"].as_array().expect("functions");
        let statuses: Vec<(&str, &str)> = functions
            .iter()
            .map(|function| {
                (
                    function["key"].as_str().unwrap_or_default(),
                    function["status"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            statuses,
            [
                ("src/main.rs::callable:main", "ok"),
                ("src/parse.rs::callable:parse_args", "ok"),
                ("src/parse.rs::callable:read_input", "missing"),
                ("src/store.rs::callable:write_result", "unreadable"),
                ("src/store.rs::callable:lock_store", "changed"),
            ]
        );
        assert_eq!(
            functions[1]["lines"],
            serde_json::json!([
                "/// Parses the flags.",
                "fn parse_args() -> Args {",
                "    Args",
                "}"
            ])
        );
        assert_eq!(functions[1]["docLines"], serde_json::json!([1, 1]));
        assert_eq!(
            (
                functions[1]["firstLine"].as_u64(),
                functions[1]["declarationLine"].as_u64()
            ),
            (Some(1), Some(2))
        );
        assert_eq!(functions[3]["reason"], "the path leaves the project");
        assert!(functions[4].get("lines").is_none());
    }

    #[test]
    fn rejects_source_requests_outside_the_catalog() {
        let project = source_fixture();
        let store = project.path().join(".tekdek");
        for (url, code) in [
            ("/source", 400),
            ("/source?flow=demo-flow", 400),
            (
                "/source?artifact=graph.json&flow=demo-flow&path=src/main.rs",
                400,
            ),
            (
                "/source?artifact=graph.json&artifact=graph.json&flow=demo-flow",
                400,
            ),
            ("/source?artifact=graph.json&flow=demo-flow&flow", 400),
            ("/source?artifact=graph.json&flow=other-flow", 404),
            ("/source?artifact=other.json&flow=demo-flow", 404),
            ("/source?artifact=..%2Fgraph.json&flow=demo-flow", 404),
            ("/source?artifact=graph.json&flow=Demo", 404),
        ] {
            assert_error(&get(&store, url), code);
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_source_behind_a_symbolic_link() {
        let project = source_fixture();
        let source = project.path().join("src");
        fs::rename(&source, project.path().join("real")).expect("move");
        std::os::unix::fs::symlink(project.path().join("real"), &source).expect("link");
        let reply = get(
            &project.path().join(".tekdek"),
            "/source?artifact=graph.json&flow=demo-flow",
        );
        let body: serde_json::Value = serde_json::from_slice(&reply.body).expect("source JSON");
        assert_eq!(body["functions"][0]["status"], "unreadable");
        assert_eq!(
            body["functions"][0]["reason"],
            "the path holds a symbolic link"
        );
    }

    #[test]
    fn project_path_accepts_only_plain_relative_paths() {
        for path in [
            "main.rs",
            "src/main.rs",
            "a/b/c.rs",
            "src/.hidden/a.rs",
            "src/...rs",
        ] {
            assert!(project_path(path), "{path} must be accepted");
        }
        for path in [
            "",
            "/src/main.rs",
            "src//main.rs",
            "src/",
            ".",
            "./main.rs",
            "src/./main.rs",
            "..",
            "../outside.rs",
            "src/../main.rs",
            "src\\main.rs",
            "C:/main.rs",
            "src/a:b.rs",
        ] {
            assert!(!project_path(path), "{path:?} must be refused");
        }
    }

    #[test]
    fn read_project_returns_the_text_or_why_it_cannot_read_a_file() {
        let root = tempfile::tempdir().expect("root");
        let root = root.path();
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        let limit = usize::try_from(MAX_SOURCE_BYTES).expect("limit");
        fs::write(root.join("limit.rs"), vec![b'a'; limit]).expect("limit file");
        fs::write(root.join("large.rs"), vec![b'a'; limit + 1]).expect("large file");
        fs::write(root.join("binary.rs"), [0xff, 0xfe, b'a']).expect("binary file");
        assert_eq!(
            read_project(root, "src/main.rs"),
            Ok("fn main() {}\n".to_owned())
        );
        assert_eq!(
            read_project(root, "limit.rs").map(|text| text.len()),
            Ok(limit)
        );
        assert_eq!(
            read_project(root, "large.rs"),
            Err("the file is larger than 2 MiB".to_owned())
        );
        assert_eq!(
            read_project(root, "binary.rs"),
            Err("the file is not UTF-8 text".to_owned())
        );
        for path in ["missing.rs", "src/missing.rs", "nowhere/missing.rs"] {
            assert_eq!(
                read_project(root, path),
                Err("the file is missing".to_owned()),
                "{path}"
            );
        }
        for path in [
            "",
            "/main.rs",
            "src//main.rs",
            "./src/main.rs",
            "../main.rs",
            "src\\main.rs",
            "C:/main.rs",
        ] {
            assert_eq!(
                read_project(root, path),
                Err("the path leaves the project".to_owned()),
                "{path:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_project_refuses_folders_links_and_pipes() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let (root, outside) = (root.path(), outside.path());
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        fs::write(outside.join("secret.rs"), "fn secret() {}\n").expect("secret");
        std::os::unix::fs::symlink(root.join("src/main.rs"), root.join("inside.rs"))
            .expect("inside link");
        std::os::unix::fs::symlink(outside.join("secret.rs"), root.join("outside.rs"))
            .expect("outside link");
        std::os::unix::fs::symlink(outside, root.join("away")).expect("folder link");
        let made = std::process::Command::new("mkfifo")
            .arg(root.join("pipe.rs"))
            .status()
            .expect("mkfifo");
        assert!(made.success(), "mkfifo must create the pipe");
        assert_eq!(
            read_project(root, "src"),
            Err("the path is not a regular file".to_owned())
        );
        assert_eq!(
            read_project(root, "pipe.rs"),
            Err("the path is not a regular file".to_owned())
        );
        for path in ["inside.rs", "outside.rs", "away/secret.rs"] {
            assert_eq!(
                read_project(root, path),
                Err("the path holds a symbolic link".to_owned()),
                "{path}"
            );
        }
    }
}
