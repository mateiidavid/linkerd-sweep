
use std::{net::SocketAddr, path::PathBuf};

use crate::admission::Admission;
use crate::tls;
use anyhow::{bail, Context, Result};
use hyper::server::conn::Http;
use kubert::shutdown;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug_span, error, info, Instrument};

#[derive(Debug)]
pub struct AdmissionServer {
    bind_addr: SocketAddr,
    shutdown: shutdown::Watch,
    cert_path: PathBuf,
    key_path: PathBuf,
}


impl AdmissionServer {
    pub fn new(bind_addr: SocketAddr, shutdown: shutdown::Watch) -> Self {
        AdmissionServer {
            bind_addr,
            shutdown,
            cert_path: PathBuf::from("/var/run/sweep/tls.crt"),
            key_path: PathBuf::from("/var/run/sweep/tls.key"),
        }
    }

    #[tracing::instrument(level = "info", skip_all)]
    pub async fn run(self) -> Result<()> {
        let listener = TcpListener::bind(&self.bind_addr)
            .await
            .expect("Failed to bind listener");

        let accept_task = tokio::spawn(AdmissionServer::accept(
            listener,
            self.cert_path.clone(),
            self.key_path.clone(),
        ));

        tokio::select! {
            _ = self.shutdown.signaled() => {
                info!("Received shutdown signal");
                return Ok(());
            }
            _ = accept_task => {},
        }

        Ok(())
    }

    /// Accept loop. Figure out how to gracefully wait until all conns have
    /// finished
    #[tracing::instrument(level = "info", skip_all)]
    async fn accept(listener: TcpListener, cert_path: PathBuf, key_path: PathBuf) {
        loop {
            let (socket, peer_addr) = match listener.accept().await {
                Ok((socket, addr)) => {
                    info!("Connection established");
                    (socket, addr)
                }
                Err(err) => {
                    error!(%err, "Failed to establish connection");
                    continue;
                }
            };

            tokio::spawn(Self::handle_conn(socket, peer_addr, cert_path.clone(), key_path.clone()));
        }
    }

    #[tracing::instrument(
        level = "info", 
        skip(socket, cert_path, key_path), 
        fields(client.addr = %client_addr))]
    async fn handle_conn(
        socket: TcpStream,
        client_addr: SocketAddr,
        cert_path: PathBuf,
        key_path: PathBuf,
    ) -> Result<()> {
        // Build TLS Connector
        let tls = match tls::mk_tls_connector(&cert_path, &key_path).await {
            Ok(tls) => tls,
            Err(error) => {
                error!(%error, "Failed to establish TLS connection");
                bail!("Failed to establish TLS connection: {}", error);
            }
        };

        // Build TLS conn
        let stream = tls.accept(socket).await.with_context(|| "TLS Error")?;
        match Http::new()
            .serve_connection(stream, Admission::new())
            .instrument(debug_span!("admission"))
            .await
        {
            Ok(_) => info!("Connection closed"),
            Err(err) => error!(%err, "Failed to process AdmissionRequest"),
        }

        Ok(())
    }
}

