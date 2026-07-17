use std::net::SocketAddr;

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    central::init_tracing();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "central listening");
    axum::serve(
        listener,
        central::app().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}
