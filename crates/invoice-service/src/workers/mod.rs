//! Background tasks. Each `spawn(state)` returns a `JoinHandle`; both are
//! idempotent and simply aborted on shutdown.

pub mod payment_sweeper;
pub mod webhook_delivery;
