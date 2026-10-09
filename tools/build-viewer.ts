// Viewer build. Bundles the viewer page script into <output directory>/app.js.
// The Cargo build script runs it so the binary embeds a bundle that matches the viewer sources.
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

const output = process.argv[2];
if (!output) throw new Error("Use node --import tsx tools/build-viewer.ts <output directory>.");
await build({
  entryPoints: [fileURLToPath(new URL("../viewer/src/main.ts", import.meta.url))],
  bundle: true, format: "esm", target: "es2022", outfile: `${output}/app.js`, logLevel: "warning",
});
