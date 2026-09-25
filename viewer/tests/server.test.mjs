// Tests for the viewer server: fixed page assets, the index allowlist, request checks, and path safety.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { request } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const script = fileURLToPath(new URL("../server.mjs", import.meta.url));
const generation = "demo-flow/generations/1-0123456789ab.json";

async function start(t, store, cwd = store, args = ["--store", store, "--port", "0"]) {
  const child = spawn(process.execPath, [script, ...args], { cwd, stdio: ["ignore", "pipe", "pipe"] });
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill();
      await once(child, "exit");
    }
  });
  const origin = await new Promise((resolve, reject) => {
    let log = "";
    const timer = setTimeout(() => reject(Error(`Server timeout: ${log}`)), 5000);
    child.once("error", reject);
    child.once("exit", () => { clearTimeout(timer); reject(Error(`Server exited: ${log}`)); });
    for (const stream of [child.stdout, child.stderr]) stream.on("data", (data) => {
      log += data;
      const match = log.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) { clearTimeout(timer); resolve(match[0]); }
    });
  });
  return { origin, child, get: (path, options) => fetch(origin + path, options) };
}

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "viewer-server-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = join(root, "artifacts");
  await mkdir(join(store, "flows", "demo-flow", "generations"), { recursive: true });
  const graph = '{"schemaVersion":1,"graph":{"nodes":[],"edges":[]}}';
  const index = { schemaVersion: 3, defaultArtifact: "graph.json", artifacts: [
    { file: "graph.json", label: "Graph", digest: "stale", flows: [
      { name: "demo-flow", file: generation, schemaVersion: 3 },
      { name: "loose", file: "demo-flow/current.json", schemaVersion: 3 },
      { name: "older", file: "demo-flow/generations/2-0123456789ab.json" },
      { name: "legacy", file: "legacy.json" },
    ] },
    { file: "same.json", flows: [] },
  ] };
  await writeFile(join(store, "graph.json"), graph);
  await writeFile(join(store, "same.json"), graph);
  await writeFile(join(store, "index.json"), JSON.stringify(index));
  await writeFile(join(store, "flows", generation), '{"schemaVersion":3}');
  await writeFile(join(store, "flows", "demo-flow", "generations", "2-0123456789ab.json"), '{"schemaVersion":3}');
  await writeFile(join(store, "flows", "demo-flow", "current.json"), "{}");
  await writeFile(join(store, "flows", "legacy.json"), '{"schemaVersion":2}');
  await writeFile(join(store, "secret.json"), '{"secret":true}');
  return { root, store, graph, index };
}

async function error(response, status) {
  assert.equal(response.status, status);
  assert.match(response.headers.get("content-type"), /^application\/json/);
  const body = await response.json();
  assert.deepEqual(Object.keys(body), ["error"]);
  assert.equal(typeof body.error, "string");
}

function raw(origin, options) {
  return new Promise((resolve, reject) => {
    const req = request(origin, options, (res) => {
      const body = [];
      res.on("data", (chunk) => body.push(chunk));
      res.on("end", () => resolve(new Response(Buffer.concat(body), { status: res.statusCode, headers: res.headers })));
    });
    req.on("error", reject);
    req.end();
  });
}

test("the server serves the viewer page, its bundle, its stylesheet, and its fonts only", async (t) => {
  const { store } = await fixture(t);
  const server = await start(t, store);
  const page = await server.get("/viewer/");
  assert.equal(page.status, 200);
  assert.match(page.headers.get("content-type"), /^text\/html/);
  const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
  assert.equal(await page.text(), html);
  assert.equal(await (await server.get("/viewer/index.html")).text(), html);
  const bundle = await server.get("/viewer/dist/app.js");
  assert.equal(bundle.status, 200);
  assert.match(bundle.headers.get("content-type"), /javascript/);
  const style = await server.get("/viewer/dist/app.css");
  assert.equal(style.status, 200);
  assert.match(style.headers.get("content-type"), /^text\/css/);
  const font = await server.get("/viewer/fonts/jetbrains-mono-latin-400.woff2");
  assert.equal(font.status, 200);
  assert.equal(font.headers.get("content-type"), "font/woff2");
  for (const path of ["/viewer", "/viewer/server.mjs", "/viewer/src/main.ts", "/viewer/styles/deck.css", "/viewer/fonts/OFL.txt",
    "/viewer/dist/other.js", "/viewer/tests/server.test.mjs", "/viewer/map.html", "/viewer/deck.html", "/AGENTS.md", "/api/session"]) {
    await error(await server.get(path), 404);
  }
});

test("the server serves the index, listed graph files, and listed schema 3 generations only", async (t) => {
  const { store, graph } = await fixture(t);
  const server = await start(t, store);
  assert.equal((await server.get("/artifacts/index.json")).status, 200);
  assert.equal(await (await server.get("/artifacts/graph.json")).text(), graph);
  assert.equal(await (await server.get("/artifacts/same.json")).text(), graph);
  assert.equal((await server.get(`/artifacts/flows/${generation}`)).status, 200);
  for (const path of ["/artifacts/secret.json", "/artifacts/flows/legacy.json", "/artifacts/flows/demo-flow/current.json",
    "/artifacts/flows/demo-flow/generations/2-0123456789ab.json", "/artifacts/notes/a.json", "/artifacts/.store.lock", "/artifacts/"]) {
    await error(await server.get(path), 404);
  }
  const head = await server.get("/artifacts/graph.json", { method: "HEAD" });
  assert.equal(head.status, 200);
  assert.equal(await head.text(), "");
  const headMissing = await server.get("/artifacts/secret.json", { method: "HEAD" });
  assert.equal(headMissing.status, 404);
  assert.equal(await headMissing.text(), "", "a HEAD error response carries no body");
  assert.ok(Number(headMissing.headers.get("content-length")) > 0);
});

test("requests require the exact Host, the same Origin, and a read method", async (t) => {
  const server = await start(t, (await fixture(t)).store);
  for (const Host of ["evil.example", "localhost:" + new URL(server.origin).port, "127.0.0.1:1"]) {
    await error(await raw(server.origin, { path: "/viewer/", headers: { Host } }), 403);
  }
  await error(await server.get("/viewer/", { headers: { Origin: "http://evil.example" } }), 403);
  await error(await raw(server.origin, { path: "/viewer/", headers: { Origin: [server.origin, server.origin] } }), 403);
  assert.equal((await server.get("/viewer/", { headers: { Origin: server.origin } })).status, 200);
  for (const method of ["PUT", "POST", "DELETE"]) await error(await server.get("/artifacts/graph.json", { method }), 405);
  const page = await server.get("/viewer/");
  assert.equal(page.headers.get("cache-control"), "no-store");
  assert.equal(page.headers.get("access-control-allow-origin"), null);
});

test("paths reject traversal, encoded separators, and symbolic links", async (t) => {
  const { root, store } = await fixture(t);
  const server = await start(t, store);
  for (const path of ["/artifacts/../secret.json", "/viewer/%2e%2e/AGENTS.md", "/artifacts/%2fetc%2fpasswd",
    "/artifacts/flows%5cdemo.json", "/artifacts/%00", "/viewer/%zz"]) {
    assert.equal((await raw(server.origin, { path })).status, 400, path);
  }
  await rm(join(store, "graph.json"));
  await symlink(join(store, "secret.json"), join(store, "graph.json"));
  await error(await server.get("/artifacts/graph.json"), 403);
  await rm(join(store, "flows"), { recursive: true });
  await symlink(root, join(store, "flows"));
  await error(await server.get(`/artifacts/flows/${generation}`), 403);
});

test("the CLI defaults to the current directory store and rejects invalid flags or a linked store", async (t) => {
  const { root, store } = await fixture(t);
  const server = await start(t, store, root, ["--port", "0"]);
  assert.equal((await server.get("/artifacts/graph.json")).status, 200);
  const linked = join(root, "linked");
  await symlink(store, linked);
  for (const args of [["--port", "-1"], ["--port", "65536"], ["--port", "1.5"], ["--port"],
    ["--unknown", "value"], ["--port", "0", "--port", "0"], ["--store", linked, "--port", "0"]]) {
    const child = spawn(process.execPath, [script, ...args], { cwd: root, stdio: ["ignore", "pipe", "pipe"] });
    let output = "";
    child.stderr.on("data", (data) => { output += data; });
    const [code] = await once(child, "exit");
    assert.equal(code, 1);
    assert.ok(output.trim());
  }
});
