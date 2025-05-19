use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;

pub async fn create_listener(
    host: &str,
    port: u16,
) -> std::io::Result<(String, tokio::net::TcpListener)> {
    if host == "*" {
        return create_wildcard_listener(port);
    }

    let addr = format!("{}:{}", host, port);
    tracing::info!("Attempting to bind server to {}...", addr);

    let tokio_listener = tokio::net::TcpListener::bind(&addr).await?;

    Ok((addr, tokio_listener))
}

fn create_ipv6_dual_stack_wildcard_listener(
    port: u16,
) -> std::io::Result<(String, tokio::net::TcpListener)> {
    let str_addr = format!("[::]:{}", port);
    let addr: SocketAddr = str_addr.parse().unwrap();

    tracing::info!(
        "Attempting to bind server to {}... (IPv6 + IPv4 dual-stack)",
        str_addr
    );

    // Try to create an IPv6 socket (this will fail if IPv6 is not supported)
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;

    // Try to make it dual-stack (this might fail on some systems)
    if let Err(e) = socket.set_only_v6(false) {
        tracing::warn!(
            "Warning: Failed to set dual-stack mode for IPv6 socket: {}. Continuing anyway.",
            e
        );
        // Continue anyway, as some systems might still work
    }

    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    // Make it non-blocking for tokio
    socket.set_nonblocking(true)?;

    // Convert to tokio listener
    let std_listener: std::net::TcpListener = socket.into();
    let tokio_listener = tokio::net::TcpListener::from_std(std_listener)?;

    Ok((str_addr, tokio_listener))
}

fn create_wildcard_listener(port: u16) -> std::io::Result<(String, tokio::net::TcpListener)> {
    // Try to create an IPv6 socket first
    // This will work on systems that support IPv6, and if it also supports dual-stack, it will bind to both IPv4 and IPv6.
    let ipv6_listener = create_ipv6_dual_stack_wildcard_listener(port);
    if ipv6_listener.is_ok() {
        return ipv6_listener;
    }

    tracing::warn!("Warning: Failed to bind IPv6 listener. Attempting IPv4 only.");

    let str_addr = format!("0.0.0.0:{}", port);
    let addr: SocketAddr = str_addr.parse().unwrap();

    tracing::info!("Attempting to bind server to {}... (IPv4)", str_addr);

    // Try to create an IPv4 socket
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;

    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    // Make it non-blocking for tokio
    socket.set_nonblocking(true)?;

    let std_listener: std::net::TcpListener = socket.into();
    let tokio_listener = tokio::net::TcpListener::from_std(std_listener)?;

    Ok((str_addr, tokio_listener))
}
