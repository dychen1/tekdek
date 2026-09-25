/* Local viewer server. This module owns the static routes of the viewer page and the read-only artifact store.
   It serves the page, its bundle and stylesheet, its fonts, the artifact index, and only the graph files and
   frozen flow generations that the index lists. It binds to the loopback address and never writes.
   # Sections
   entry point, routes, index allowlist, safe reads */
import { constants } from "node:fs";
import { lstat, mkdir, open, realpath } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const viewer = dirname(fileURLToPath(import.meta.url));
const types = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".json": "application/json; charset=utf-8", ".woff2": "font/woff2" };
const responseHeaders = { "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff", "Cross-Origin-Resource-Policy": "same-origin", "Referrer-Policy": "no-referrer" };

// ============ entry point ============

async function main() {
  let store = resolve("artifacts"), port = 8000;
  const args = process.argv.slice(2);
  const seen = new Set();
  for (let i = 0; i < args.length; i += 2) {
    const flag = args[i], value = args[i + 1];
    if (!["--store", "--port"].includes(flag) || !value || seen.has(flag)) {
      throw new Error("Use node viewer/server.mjs --store artifacts --port 8000.");
    }
    seen.add(flag);
    if (flag === "--store") store = resolve(value);
    else {
      if (!/^\d+$/.test(value) || Number(value) > 65535) throw new Error("The port must be between 0 and 65535.");
      port = Number(value);
    }
  }
  await safePath(store, []);
  await mkdir(store, { recursive: true });
  store = await realpath(store);
  let origin, host;
  const server = createServer({ requestTimeout: 15000, headersTimeout: 10000 }, async (req, res) => {
    try {
      for (const name of ["host", "origin"]) {
        if ((req.headersDistinct[name]?.length ?? 0) > 1) fail(403, "Duplicate security header.");
      }
      if (req.headers.host !== host) fail(403, "Unexpected Host header.");
      if (req.headers.origin !== undefined && req.headers.origin !== origin) fail(403, "Cross-origin requests are not allowed.");
      if (req.method !== "GET" && req.method !== "HEAD") fail(405, "Use GET or HEAD.");
      const { root, file } = await route(routeParts(req.url), store);
      const bytes = await readSafe(root, file.split("/"));
      res.writeHead(200, { ...responseHeaders, "Content-Type": types[extname(file)], "Content-Length": bytes.length });
      res.end(req.method === "HEAD" ? undefined : bytes);
    } catch (error) {
      req.resume();
      const status = error.status ?? (error.code === "ENOENT" ? 404 : error.code === "ELOOP" ? 403 : 500);
      send(res, status, { error: error.status ? error.message : status === 404 ? "File not found." : "Cannot access the viewer store." }, req.method === "HEAD");
    }
  });
  server.on("error", (error) => { console.error(error.message); process.exitCode = 1; });
  server.listen(port, "127.0.0.1", () => {
    host = `127.0.0.1:${server.address().port}`;
    origin = `http://${host}`;
    console.log(`${origin}/viewer/`);
  });
}

// ============ routes ============

/** Returns the root and the relative file of an allowed request path. Any other path is a 404. */
async function route(parts, store) {
  const [top, ...rest] = parts;
  if (top === "viewer") {
    if (rest.length === 1 && (rest[0] === "" || rest[0] === "index.html")) return { root: viewer, file: "index.html" };
    if (rest.length === 2 && rest[0] === "dist" && ["app.js", "app.css"].includes(rest[1])) return { root: viewer, file: `dist/${rest[1]}` };
    if (rest.length === 2 && rest[0] === "fonts" && /^[a-z0-9-]+\.woff2$/.test(rest[1])) return { root: viewer, file: `fonts/${rest[1]}` };
  } else if (top === "artifacts" && rest.length) {
    const file = rest.join("/");
    if (file === "index.json" || listed(await readIndex(store), file)) return { root: store, file };
  }
  fail(404, "File not found.");
}

/** Splits a request path into decoded components. Traversal, separators, and control characters are rejected. */
function routeParts(url) {
  const raw = url.split("?")[0];
  if (!raw.startsWith("/") || raw.startsWith("//")) fail(400, "Invalid request path.");
  return raw.slice(1).split("/").map((part) => {
    let decoded;
    try { decoded = decodeURIComponent(part); } catch { fail(400, "Invalid path encoding."); }
    if (decoded === "." || decoded === ".." || /[\x00-\x1f\x7f/\\]/.test(decoded)) fail(400, "Invalid request path.");
    return decoded;
  });
}

// ============ index allowlist ============

async function readIndex(store) {
  return JSON.parse((await readSafe(store, ["index.json"])).toString("utf8"));
}

/** True when the index lists the file: a graph file, or a frozen schema 3 generation under `flows/`. */
function listed(index, file) {
  return (index.artifacts ?? []).some((entry) => (graphName(entry.file) && file === entry.file) ||
    (entry.flows ?? []).some((flow) => generationFile(flow) && file === `flows/${flow.file}`));
}

/** Accepts a graph file name: one path component that ends in `.json`, other than the index. */
function graphName(name) {
  return typeof name === "string" && /^[^./\\\x00-\x1f][^/\\\x00-\x1f]*\.json$/.test(name) && name !== "index.json";
}

/** Accepts a schema 3 flow generation path, `<flowId>/generations/<ordinal>-<digest12>.json`, from an index entry. */
function generationFile(flow) {
  if (flow?.schemaVersion !== 3 || typeof flow.file !== "string") return false;
  const [id, folder, name, ...rest] = flow.file.split("/");
  return rest.length === 0 && folder === "generations" && /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(id ?? "") &&
    /^[1-9][0-9]{0,8}-[a-f0-9]{12}\.json$/.test(name ?? "");
}

// ============ safe reads ============

/** Writes one JSON response. A HEAD response carries the headers only. */
function send(res, status, value, headOnly = false) {
  const body = JSON.stringify(value);
  res.writeHead(status, { ...responseHeaders, "Content-Type": types[".json"], "Content-Length": Buffer.byteLength(body) });
  res.end(headOnly ? undefined : body);
}

function fail(status, message) {
  throw Object.assign(new Error(message), { status });
}

/** Rejects symbolic links in the root and each relative path component. */
async function safePath(root, parts) {
  let path = root;
  for (const part of [null, ...parts]) {
    if (part !== null) path = join(path, part);
    try {
      if ((await lstat(path)).isSymbolicLink()) fail(403, "Symbolic links are not allowed.");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  return path;
}

/** Reads one regular file without following a symbolic link. */
async function readSafe(root, parts) {
  const path = await safePath(root, parts);
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!(await file.stat()).isFile()) fail(403, "The path must be a regular file.");
    return await file.readFile();
  } finally { await file.close(); }
}

main().catch((error) => { console.error(error.message); process.exitCode = 1; });
