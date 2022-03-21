use std::net::SocketAddr;

use tokio::net::TcpListener;

// Steps:
// 1. Bind TCP using tokio, accept
// 2. Once connection is established, do TLS
// 3. Serve

pub struct AdmissionServer {
    client: kube::client::Client,
    bind_addr: SocketAddr,
}

impl AdmissionServer {
    pub fn new(client: kube::client::Client, bind_addr: SocketAddr) -> Self {
        Self { client, bind_addr }
    }

    pub async fn run(self) {
        let listener = TcpListener::bind(&self.bind_addr)
            .await
            .expect("listener should be created successfully");
        let local_addr = listener.local_addr().expect("can't get local addr");

        loop {
            let socket = match listener.accept().await {
                Ok((socket, _)) => socket,
                Err(err) => {
                    tracing::error!(%err, "Failed to accept connection");
                    continue;
                }
            };

            let client_addr = match socket.peer_addr() {
                Ok(addr) => addr,
                Err(err) => {
                    tracing::error!(%err, "Failed to get peer addr");
                    continue;
                }
            };
        }
    }
}
