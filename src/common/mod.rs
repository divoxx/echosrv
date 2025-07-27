//! Common traits and types used across the echosrv library
//!
//! This module contains the core traits that define the interface
//! for echo servers and clients.

pub mod test_utils;
pub mod traits;

pub use test_utils::create_controlled_test_server_with_limit;
pub use traits::{EchoClient, EchoServerTrait};
