//! Library root. The binary is a thin shell over this crate so tests can use it
//! directly.

pub mod app;
pub mod auth;
pub mod config;
pub mod customers;
pub mod error;
pub mod health;
pub mod invoice_state;
pub mod invoices;
pub mod money;
pub mod outbox;
pub mod pagination;
pub mod payments;
pub mod psp;
pub mod sweeper;
pub mod telemetry;
