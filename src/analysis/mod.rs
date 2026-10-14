//! Source analysis.
//!
//! This module owns the indexer: it turns one codebase root into one code graph.
//! A pipeline configuration is the input. A graph is the output.
//!
//! # Modules
//!
//! `pipeline` runs the stages. `preflight` selects languages and checks their
//! server programs, and `program` finds those programs. `source`, `language`,
//! `lsp`, `fact`, and `graph` hold the stages, `config` loads the language
//! profiles, and `timing` reports stage and request times on request. The
//! index starts no language server: `resolve` asks the servers for the
//! semantic edges of some nodes on demand.
//!
//! # Stages
//!
//! 1. Load documents for every enabled profile.
//! 2. Detect languages.
//! 3. Run the syntax step of each detected adapter on its own thread and
//!    collect the fact sets in language order.
//! 4. Build one graph from those facts, and hash the content of each entity.

pub mod config;
pub mod fact;
pub mod graph;
pub mod language;
#[cfg(any(feature = "python", feature = "rust"))]
pub mod lsp;
#[cfg(any(feature = "python", feature = "rust"))]
mod pipeline;
#[cfg(any(feature = "python", feature = "rust"))]
mod preflight;
pub mod program;
#[cfg(any(feature = "python", feature = "rust"))]
pub mod resolve;
pub mod source;
pub(crate) mod timing;

#[cfg(any(feature = "python", feature = "rust"))]
pub use pipeline::{
    Indexed, PipelineConfig, PipelineError, detect_project_languages, run, source_hash,
};
pub use program::{find_program, find_sibling_program};
