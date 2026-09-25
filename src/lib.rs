//! Code graph analysis and flows.
//!
//! This crate indexes a codebase into one graph artifact, freezes agent-written
//! flows with clusters against that graph, and keeps the catalog that the viewer
//! reads.
//!
//! # Modules
//!
//! `analysis` turns a codebase root into a graph.
//! `store` owns the store folder and the graph artifact.
//! `flow` imports and verifies frozen flows.
//! `catalog` writes `index.json` for the viewer.

pub mod analysis;
pub mod catalog;
pub mod flow;
pub mod store;
