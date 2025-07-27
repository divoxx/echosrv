//! Performance optimization components

pub mod buffer_pool;

pub use buffer_pool::{BufferPool, PoolStats, PooledBuffer, global_pool, init_global_pool};
