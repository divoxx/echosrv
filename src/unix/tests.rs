use crate::common::{EchoClient, EchoServerTrait};
use crate::unix::{
    UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixStreamConfig,
    UnixStreamEchoClient, UnixStreamEchoServer,
};
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn test_unix_stream_echo() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_stream.sock");

    let config = UnixStreamConfig::default().with_socket_path(socket_path.clone());

    let server = UnixStreamEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();

    // Start server in background
    let server_handle = tokio::spawn(async move { server.run().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test client
    let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();

    // Test string echo
    let test_string = "Hello, Unix Stream!";
    let response = client.echo_string(test_string).await.unwrap();
    assert_eq!(response, test_string);

    // Test binary data echo
    let test_data = b"Binary data with \x00 null bytes";
    let response = client.echo(test_data).await.unwrap();
    assert_eq!(response, test_data);

    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_unix_datagram_echo() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_datagram.sock");

    let config = UnixDatagramConfig::default().with_socket_path(socket_path.clone());

    let server = UnixDatagramEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();

    // Start server in background
    let server_handle = tokio::spawn(async move { server.run().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test client with timeout
    let client_result = tokio::time::timeout(Duration::from_secs(10), async {
        let mut client = UnixDatagramEchoClient::connect(socket_path).await.unwrap();

        // Test string echo
        let test_string = "Hello, Unix Datagram!";
        let response = client.echo_string(test_string).await.unwrap();
        assert_eq!(response, test_string);

        // Test binary data echo
        let test_data = b"Binary data with \x00 null bytes";
        let response = client.echo(test_data).await.unwrap();
        assert_eq!(response, test_data);
    })
    .await;

    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();

    // Check if client test passed
    client_result.unwrap();
}

#[tokio::test]
async fn test_unix_stream_multiple_clients() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_multi_stream.sock");

    let config = UnixStreamConfig::default().with_socket_path(socket_path.clone());

    let server = UnixStreamEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();

    // Start server in background
    let server_handle = tokio::spawn(async move { server.run().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test multiple concurrent clients
    let mut handles = Vec::new();

    for i in 0..5 {
        let socket_path = socket_path.clone();
        let handle = tokio::spawn(async move {
            let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();
            let test_string = format!("Client {i}");
            let response = client.echo_string(&test_string).await.unwrap();
            assert_eq!(response, test_string);
        });
        handles.push(handle);
    }

    // Wait for all clients to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_unix_stream_large_data() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_large_stream.sock");

    let config = UnixStreamConfig::default().with_socket_path(socket_path.clone());

    let server = UnixStreamEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();

    // Start server in background
    let server_handle = tokio::spawn(async move { server.run().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test client with large data
    let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();

    // Create large test data (larger than buffer size)
    let large_data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
    let response = client.echo(&large_data).await.unwrap();
    assert_eq!(response, large_data);

    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_socket_file_removed_on_shutdown_and_restart_works() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("restart.sock");

    for _ in 0..2 {
        let server = UnixStreamEchoServer::new(
            UnixStreamConfig::default().with_socket_path(socket_path.clone()),
        );
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.unwrap();
        assert_eq!(bound.local_addr().as_unix(), Some(&socket_path));
        let handle = tokio::spawn(bound.serve());

        let mut client = UnixStreamEchoClient::connect(socket_path.clone())
            .await
            .unwrap();
        assert_eq!(client.echo_string("again").await.unwrap(), "again");

        shutdown.send(()).unwrap();
        handle.await.unwrap().unwrap();
        assert!(!socket_path.exists(), "socket file not cleaned up");
    }
}

#[tokio::test]
async fn test_stale_socket_file_is_recovered() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket_path).unwrap());
    assert!(socket_path.exists());

    let server = UnixStreamEchoServer::new(
        UnixStreamConfig::default().with_socket_path(socket_path.clone()),
    );
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await.unwrap().serve());
    let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();
    assert_eq!(client.echo_string("ok").await.unwrap(), "ok");
    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_inherited_socket_file_is_not_removed() {
    use crate::network::{BindStrategy, InheritedFd};
    use std::os::fd::OwnedFd;

    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("inherited.sock");
    let parent_listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

    let config = UnixStreamConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(parent_listener))),
        ..UnixStreamConfig::default()
    };
    let server = UnixStreamEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await.unwrap().serve());

    let mut client = UnixStreamEchoClient::connect(socket_path.clone())
        .await
        .unwrap();
    assert_eq!(client.echo_string("inherited").await.unwrap(), "inherited");

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
    assert!(
        socket_path.exists(),
        "inherited socket file must not be removed"
    );
}

#[tokio::test]
async fn test_unix_stream_connection_limit() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("limit.sock");
    let config = UnixStreamConfig {
        max_connections: 1,
        ..UnixStreamConfig::default().with_socket_path(socket_path.clone())
    };
    let server = UnixStreamEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await.unwrap().serve());

    let mut first = UnixStreamEchoClient::connect(socket_path.clone())
        .await
        .unwrap();
    assert_eq!(first.echo_string("one").await.unwrap(), "one");

    let mut second = UnixStreamEchoClient::connect(socket_path.clone())
        .await
        .unwrap();
    let rejected = second.echo_string("two").await;
    assert!(!matches!(rejected, Ok(ref s) if s == "two"));

    drop(first);
    // The slot frees once the server notices the disconnect.
    let mut ok = false;
    for _ in 0..50 {
        let mut third = UnixStreamEchoClient::connect(socket_path.clone())
            .await
            .unwrap();
        if matches!(third.echo_string("three").await, Ok(ref s) if s == "three") {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ok, "connection slot was not released");

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_generic_datagram_server_honors_unix_config() {
    use crate::datagram::DatagramEchoServer;
    use crate::unix::UnixDatagramProtocol;

    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("generic.sock");
    let config = UnixDatagramConfig::default().with_socket_path(socket_path.clone());
    let server: DatagramEchoServer<UnixDatagramProtocol> = DatagramEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr().as_unix(), Some(&socket_path));
    let handle = tokio::spawn(bound.serve());

    let mut client = UnixDatagramEchoClient::connect(socket_path.clone())
        .await
        .unwrap();
    let client_path = client.local_path().unwrap().to_path_buf();
    assert!(client_path.exists());
    // macOS caps Unix datagrams at its default SO_SNDBUF (2 KiB).
    let payload = vec![7u8; 1500];
    assert_eq!(client.echo(&payload).await.unwrap(), payload);
    drop(client);
    assert!(
        !client_path.exists(),
        "client temp socket not removed on drop"
    );

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
    assert!(!socket_path.exists());
}
