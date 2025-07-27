#[cfg(test)]
mod tests {
    use crate::common::traits::EchoServerTrait;
    use crate::{TcpConfig, TcpEchoServer};
    use std::time::Duration;

    #[tokio::test]
    async fn test_config_default() {
        let config = TcpConfig::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.buffer_size, 1024);
        // Timeouts are now always set (Duration instead of Option<Duration>)
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_echo_server_new() {
        let config = TcpConfig::default();
        let server = TcpEchoServer::new(config.into());
        assert!(server.shutdown_signal().receiver_count() == 0);
    }
}
