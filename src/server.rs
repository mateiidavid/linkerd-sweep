use core::task;
use std::{net::SocketAddr, path::PathBuf};

use crate::tls;
use anyhow::{anyhow, Context, Result};
use futures::{future, TryFutureExt};
use hyper::{server::conn::Http, service::Service, Response};
use hyper::{Body, Request};
use kube::core::{admission::AdmissionReview, DynamicObject};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info_span, Instrument};

pub struct AdmissionServer {
    client: kube::client::Client,
    bind_addr: SocketAddr,
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl AdmissionServer {
    pub fn new(client: kube::client::Client, bind_addr: SocketAddr) -> Self {
        Self {
            client,
            bind_addr,
            cert_path: PathBuf::from("/var/run/sweep/tls.crt"),
            key_path: PathBuf::from("/var/run/sweep/tls.key"),
        }
    }

    pub async fn run(self) {
        tracing::info!("running, boss");
        let listener = TcpListener::bind(&self.bind_addr)
            .await
            .expect("listener should be created successfully");
        let _local_addr = listener.local_addr().expect("can't get local addr");

        loop {
            tracing::info!("loopin, boss");
            let socket = match listener.accept().await {
                Ok((socket, _)) => socket,
                Err(err) => {
                    tracing::error!(%err, "Failed to accept connection");
                    continue;
                }
            };

            let peer_addr = match socket.peer_addr() {
                Ok(addr) => addr,
                Err(err) => {
                    tracing::error!(%err, "Failed to get peer addr");
                    continue;
                }
            };

            tokio::spawn(
                Self::handle_conn(
                    socket,
                    self.client.clone(),
                    self.cert_path.clone(),
                    self.key_path.clone(),
                )
                .map_err(|err| tracing::error!(%err))
                .instrument(info_span!("connection", peer.addr = %peer_addr)),
            );
        }
    }

    async fn handle_conn(
        socket: TcpStream,
        client: kube::client::Client,
        cert_path: PathBuf,
        key_path: PathBuf,
    ) -> Result<()> {
        tracing::info!("buildin a conn");
        // Build TLS Connector
        let tls = match tls::mk_tls_connector(&cert_path, &key_path).await {
            Ok(tls) => tls,
            Err(err) => {
                tracing::error!(%err, "failed to build tls connector");
                return Err(anyhow!("could not establish TLS connection"));
            }
        };
        // Build TLS conn
        let stream = tls.accept(socket).await.with_context(|| "TLS Error")?;
        match Http::new()
            .serve_connection(stream, Handler { client })
            .await
        {
            Ok(_) => todo!(),
            Err(err) => tracing::error!(%err, "failed to process admission request"),
        }

        Ok(())
    }
}

#[derive(Clone)]
struct Handler {
    client: kube::client::Client,
}

impl Service<Request<Body>> for Handler {
    type Response = Response<Body>;

    type Error = anyhow::Error;

    type Future = future::BoxFuture<'static, anyhow::Result<Response<Body>>>;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        tracing::info!(?req, "got a request, boss");
        Box::pin(async {
            tracing::info!("PROCESSING STARTED");
            let review: AdmissionReview<DynamicObject> = {
                let bytes = hyper::body::to_bytes(req.into_body())
                    .await
                    .with_context(|| "Failed to convert request body to bytes")?;
                tracing::info!(?bytes, "BYTEZ");
                serde_json::from_slice(&bytes)
                    .with_context(|| "Failed to deserialize request body bytes to json")?
            };
            Ok(Response::builder().status(200).body(Body::empty()).unwrap())
        })
    }
}
