//! Executes a `Plan` against a `sql::engine::Engine`.

mod aggregate;
mod execute;
mod join;
mod transform;
mod write;
mod source;

pub use execute::{ExecutionResult, execute_plan};