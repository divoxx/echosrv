use bytes::{Bytes, BytesMut};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

/// A pool of reusable buffers to reduce allocations
#[derive(Debug)]
pub struct BufferPool {
    pool: Arc<Mutex<VecDeque<BytesMut>>>,
    buffer_size: usize,
    max_pool_size: usize,
}

impl BufferPool {
    /// Create a new buffer pool
    pub fn new(buffer_size: usize, max_pool_size: usize) -> Self {
        Self {
            pool: Arc::new(Mutex::new(VecDeque::with_capacity(max_pool_size))),
            buffer_size,
            max_pool_size,
        }
    }

    /// Get a buffer from the pool or create a new one
    pub fn get(&self) -> PooledBuffer {
        let buffer = {
            let mut pool = self.pool.lock().unwrap();
            pool.pop_front().unwrap_or_else(|| BytesMut::with_capacity(self.buffer_size))
        };

        PooledBuffer {
            buffer: Some(buffer),
            pool: Arc::clone(&self.pool),
            max_pool_size: self.max_pool_size,
        }
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        let pool = self.pool.lock().unwrap();
        PoolStats {
            available_buffers: pool.len(),
            buffer_size: self.buffer_size,
            max_pool_size: self.max_pool_size,
        }
    }
}

/// A buffer that returns to the pool when dropped
pub struct PooledBuffer {
    buffer: Option<BytesMut>,
    pool: Arc<Mutex<VecDeque<BytesMut>>>,
    max_pool_size: usize,
}

impl PooledBuffer {
    /// Get mutable access to the underlying buffer
    pub fn get_mut(&mut self) -> &mut BytesMut {
        self.buffer.as_mut().unwrap()
    }

    /// Get immutable access to the underlying buffer
    pub fn get(&self) -> &BytesMut {
        self.buffer.as_ref().unwrap()
    }

    /// Convert to bytes, consuming the buffer
    pub fn freeze(mut self) -> Bytes {
        let buffer = self.buffer.take().unwrap();
        buffer.freeze()
    }

    /// Clear the buffer and reset its length
    pub fn clear(&mut self) {
        if let Some(ref mut buffer) = self.buffer {
            buffer.clear();
        }
    }

    /// Get the current length of the buffer
    pub fn len(&self) -> usize {
        self.buffer.as_ref().map_or(0, |b| b.len())
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the capacity of the buffer
    pub fn capacity(&self) -> usize {
        self.buffer.as_ref().map_or(0, |b| b.capacity())
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            // Clear the buffer and return it to the pool if there's space
            buffer.clear();
            
            let mut pool = self.pool.lock().unwrap();
            if pool.len() < self.max_pool_size {
                pool.push_back(buffer);
            }
            // If pool is full, just drop the buffer
        }
    }
}

impl std::ops::Deref for PooledBuffer {
    type Target = BytesMut;

    fn deref(&self) -> &Self::Target {
        self.buffer.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_mut().unwrap()
    }
}

/// Statistics about buffer pool usage
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub available_buffers: usize,
    pub buffer_size: usize,
    pub max_pool_size: usize,
}

/// Global buffer pool for the application
static GLOBAL_POOL: std::sync::OnceLock<BufferPool> = std::sync::OnceLock::new();

/// Get the global buffer pool instance
pub fn global_pool() -> &'static BufferPool {
    GLOBAL_POOL.get_or_init(|| {
        BufferPool::new(8192, 100) // 8KB buffers, up to 100 in pool
    })
}

/// Initialize the global buffer pool with custom settings
pub fn init_global_pool(buffer_size: usize, max_pool_size: usize) -> Result<(), &'static str> {
    GLOBAL_POOL.set(BufferPool::new(buffer_size, max_pool_size))
        .map_err(|_| "Global pool already initialized")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_pool_basic() {
        let pool = BufferPool::new(1024, 5);
        
        // Get a buffer
        let mut buffer = pool.get();
        assert_eq!(buffer.capacity(), 1024);
        assert!(buffer.is_empty());
        
        // Use the buffer
        buffer.extend_from_slice(b"hello");
        assert_eq!(buffer.len(), 5);
        
        // Stats should show no available buffers (one is checked out)
        let stats = pool.stats();
        assert_eq!(stats.available_buffers, 0);
        
        // Drop the buffer (returns to pool)
        drop(buffer);
        
        // Stats should show one available buffer
        let stats = pool.stats();
        assert_eq!(stats.available_buffers, 1);
    }

    #[test]
    fn test_buffer_pool_reuse() {
        let pool = BufferPool::new(1024, 5);
        
        // Get and use a buffer
        {
            let mut buffer = pool.get();
            buffer.extend_from_slice(b"test data");
        } // buffer returns to pool here
        
        // Get another buffer - should be the same one, but cleared
        let buffer = pool.get();
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), 1024);
    }

    #[test]
    fn test_buffer_pool_max_size() {
        let pool = BufferPool::new(1024, 2);
        
        // Create more buffers than the pool can hold
        let _buffer1 = pool.get();
        let _buffer2 = pool.get();
        let _buffer3 = pool.get();
        
        // All buffers are checked out
        assert_eq!(pool.stats().available_buffers, 0);
        
        // Drop all buffers
        drop(_buffer1);
        drop(_buffer2);
        drop(_buffer3);
        
        // Pool should only have 2 buffers (its max size)
        assert_eq!(pool.stats().available_buffers, 2);
    }

    #[test]
    fn test_global_pool() {
        // This might interfere with other tests, so be careful
        let pool = global_pool();
        let buffer = pool.get();
        assert!(buffer.capacity() > 0);
    }
}