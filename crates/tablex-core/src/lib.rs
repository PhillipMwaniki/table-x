//! # tablex-core
//!
//! Database-agnostic foundations for Table X: the dynamic [`value::Value`] model,
//! the [`driver::Driver`] / [`driver::Connection`] contract, schema description types,
//! and the normalized [`error::Error`].
//!
//! This crate deliberately has no dependency on Tauri, on any GUI toolkit, or on any
//! specific database. It is the seam that lets drivers be tested headlessly and lets
//! the same logic back a future CLI or server without a rewrite.

pub mod config;
pub mod driver;
pub mod error;
pub mod export;
pub mod registry;
pub mod result;
pub mod schema;
pub mod sql;
pub mod value;

pub use config::ConnectionConfig;
pub use driver::{Capabilities, Connection, Driver, DriverInfo, FetchOptions, RowEdit};
pub use error::{Error, ErrorCategory, ErrorPayload, Result};
pub use result::{Column, QueryOutcome, ResultSet, StatementResult};
pub use schema::{SchemaNode, TableDetail};
pub use value::{Value, ValueKind};
