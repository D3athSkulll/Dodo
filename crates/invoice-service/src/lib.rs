//! Invoice & payment service.
//!
//! Layout:
//!
//! ```text
//! money            domain money type (integer cents)
//! config           process configuration from the environment
//! error            the single `ApiError` and its JSON shape
//! telemetry        logging + the per-request id middleware
//! secret           random secret generation
//! pagination       keyset cursor + list envelope
//! state            `AppState`, the pool, the HTTP client
//! auth             API-key middleware + the `Business` extractor
//! psp              outbound client for the payment processor
//!
//! routes/          HTTP handlers, one module per resource, plus `router()`
//! domain/          business rules: the invoice state machine, the webhook outbox
//! workers/         background tasks: payment reconciliation, webhook delivery
//! ```
//!
//! The binary (`main.rs`) is a thin shell over this crate so the integration
//! tests can run it in-process.

pub mod auth;
pub mod config;
pub mod error;
pub mod money;
pub mod pagination;
pub mod psp;
pub mod secret;
pub mod state;
pub mod telemetry;

pub mod domain;
pub mod routes;
pub mod workers;
