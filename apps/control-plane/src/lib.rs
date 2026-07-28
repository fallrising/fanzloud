//! Private single-operator P0 control-plane boundary.
//!
//! T005B owns bounded HTTP parsing, application authentication, Origin enforcement,
//! process-instance idempotency, typed redacted responses, and detached blocking composition over
//! the accepted login broker and P0 session runtime.

mod config;
mod error;
mod idempotency;
mod ports;
mod routes;
mod state;
mod transport;
mod types;

pub use config::{
    OperatorBootstrapToken, P0HttpConfig, P0HttpConfigError, P0HttpConfigField, P0PublicOrigin,
};
pub use error::{P0HttpShutdownError, P0HttpShutdownErrorCategory};
pub use state::P0ControlPlane;

#[cfg(test)]
mod tests;
