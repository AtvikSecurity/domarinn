//! The signal that ends [`crate::serve_on`].

/// Resolve when the process is asked to stop, logging which signal asked.
///
/// SIGTERM matters as much as Ctrl-C: it is what `docker stop` and a
/// Kubernetes pod termination send, and the image runs `domarinn` as PID 1
/// with no init. The kernel gives PID 1 no default signal disposition, so an
/// unhandled SIGTERM is simply ignored until the grace period ends in a
/// SIGKILL — mid-write. Handling it here lets axum drain in-flight requests
/// (and so finish their SQLite transactions) before the process exits.
pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "could not listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let which = tokio::select! {
        () = ctrl_c => "SIGINT",
        () = terminate => "SIGTERM",
    };
    tracing::info!(signal = which, "shutting down: draining in-flight requests");
}
