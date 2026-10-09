//! Viewer bundle build.
//!
//! This build script bundles the viewer script into `OUT_DIR/viewer/app.js`,
//! so the binary always embeds a bundle that matches the viewer sources. Node.js
//! and the locked npm dependencies (`npm ci`) are the inputs.

use std::{env, path::PathBuf, process::Command};

fn main() {
    for input in [
        "viewer/src",
        "tools/build-viewer.ts",
        "package-lock.json",
        "tsconfig.json",
    ] {
        println!("cargo::rerun-if-changed={input}");
    }
    if let Err(message) = bundle() {
        println!("cargo::error={message}");
        std::process::exit(1);
    }
}

/// Runs the esbuild script of the viewer.
fn bundle() -> Result<(), String> {
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR is not set")?);
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?).join("viewer");
    let modules = root.join("node_modules");
    if !modules.join("esbuild").is_dir() || !modules.join("tsx").is_dir() {
        return Err(format!(
            "the viewer bundle needs the npm dependencies: run `npm ci` in {}, then build again",
            root.display()
        ));
    }
    let status = Command::new("node")
        .args(["--import", "tsx"])
        .arg(root.join("tools").join("build-viewer.ts"))
        .arg(&output)
        .current_dir(&root)
        .status()
        .map_err(|error| {
            format!("cannot run node ({error}): install Node.js 22 or later, then build again")
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the viewer bundle build failed ({status})"))
    }
}
