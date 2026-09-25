// Viewer build. Bundles the viewer page script and copies its stylesheet into viewer/dist.
import { build } from "esbuild";
import { copyFile, mkdir } from "node:fs/promises";

await mkdir("viewer/dist", { recursive: true });
await build({ entryPoints: ["viewer/src/main.ts"], bundle: true, format: "esm", target: "es2022", outfile: "viewer/dist/app.js" });
await copyFile("viewer/styles/deck.css", "viewer/dist/app.css");
