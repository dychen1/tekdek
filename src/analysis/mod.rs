//! Source analysis.
//!
//! This module owns the indexer: it turns one codebase root into one code graph.
//! A pipeline configuration is the input. A graph and its detected languages are
//! the output.
//!
//! # Modules
//!
//! `pipeline` runs the stages. `preflight` selects languages and checks their
//! server programs. `source`, `language`, `lsp`, `fact`, and `graph` hold the
//! stages, and `config` loads the language profiles.
//!
//! # Stages
//!
//! 1. Load documents for every enabled profile.
//! 2. Detect languages and preflight their server programs.
//! 3. Run each detected adapter and collect its fact set.
//! 4. Build one graph from those facts, and hash the content of each entity.

pub mod config;
pub mod fact;
pub mod graph;
pub mod language;
pub mod lsp;
#[cfg(any(feature = "python", feature = "rust"))]
mod pipeline;
#[cfg(any(feature = "python", feature = "rust"))]
mod preflight;
pub mod source;

#[cfg(any(feature = "python", feature = "rust"))]
pub use pipeline::{
    PipelineConfig, PipelineError, PipelineOutputError, PipelineResult, run, run_and_write,
};
