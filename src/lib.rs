//! Code graph analysis and flows.
//!
//! This crate indexes a codebase into one graph artifact, freezes agent-written
//! flows with clusters against that graph, and builds the catalog that the
//! viewer reads.
//!
//! # Modules
//!
//! `analysis` turns a codebase root into a graph.
//! `store` owns the store folder and the graph artifact.
//! `flow` imports and verifies frozen flows.
//! `catalog` builds the catalog of graphs and flows for the viewer.
//! `query` searches graph nodes and lists their neighbors for agents.
//! `serve` serves the embedded viewer and the listed files of one store.
//! `source_view` cuts the source of the step functions of one flow for the viewer.

pub mod analysis;
pub mod catalog;
pub mod flow;
pub mod query;
pub mod serve;
pub mod source_view;
pub mod store;
