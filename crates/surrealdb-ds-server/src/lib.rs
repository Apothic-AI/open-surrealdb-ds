//! The server binary's reusable pieces, as a library.
//!
//! The binary itself is thin — it builds a registry, constructs, and otherwise
//! hands argv to the upstream CLI — but the composer in [`composer`] is the one
//! place our engine is handed to the rest of SurrealDB (ADR-0003), so it is worth
//! being able to name from a test. The golden-file harness
//! (`tests/golden.rs`) needs exactly the same registry the binary serves from, and
//! a second copy of it would be a second place to change when our schemes change.
//!
//! This is a library target so that [`composer`] is reachable from an integration
//! test. It links `surrealdb-server`, so it is BUSL-1.1 like the binary; see
//! ../NOTICE and DECISIONS.md ADR-0002.

pub mod composer;

pub use composer::{DsComposer, registry};