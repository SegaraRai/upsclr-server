use tokio::signal;
use tracing::info;

pub async fn shutdown_signal(ctrl_c_twice: bool) {
    let ctrl_c = async {
        // Wait for Ctrl-C signal
        signal::ctrl_c().await.expect("failed to listen for Ctrl-C");

        if ctrl_c_twice {
            info!("Received Ctrl-C, press Ctrl-C again to exit");

            // Wait for Ctrl-C again to confirm exit
            signal::ctrl_c()
                .await
                .expect("failed to listen for Ctrl-C again");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
