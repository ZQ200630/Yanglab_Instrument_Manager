//! Native experiment orchestration; no instrument transport or GUI dependency.
mod model;
mod runner;
mod templates;
pub use model::*;
pub use runner::*;
pub use templates::*;
