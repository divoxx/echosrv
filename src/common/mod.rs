//! Common traits and types used across the echosrv library
//!
//! This module contains the core traits that define the interface
//! for echo servers and clients.

pub(crate) mod lifecycle;
pub mod traits;

pub use traits::{EchoClient, EchoServerTrait};
