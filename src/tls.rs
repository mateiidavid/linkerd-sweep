use anyhow::{anyhow, Result};
use std::{io::BufReader, path::PathBuf, sync::Arc};
use tokio::fs;

pub async fn load_certificate(path: &PathBuf) -> Result<Vec<rustls::Certificate>> {
    let file = fs::read(path).await?;
    let mut reader = BufReader::new(file.as_slice());

    let certs = rustls_pemfile::certs(&mut reader)?;
    if certs.len() == 0 {
        return Err(anyhow!(
            "expected at least one certificate in file {:?}",
            &path
        ));
    }

    Ok(certs.into_iter().map(rustls::Certificate).collect())
}

pub async fn load_keys(path: &PathBuf) -> Result<rustls::PrivateKey> {
    let file = fs::read(path).await?;
    let mut reader = BufReader::new(file.as_slice());

    let mut keys = rustls_pemfile::rsa_private_keys(&mut reader)?;
    if keys.len() != 1 {
        return Err(anyhow!("expected only one private key in file {:?}", &path));
    }

    Ok(rustls::PrivateKey(keys.remove(0)))
}

pub async fn mk_tls_connector(
    cert_path: &PathBuf,
    key_path: &PathBuf,
) -> Result<tokio_rustls::TlsAcceptor> {
    let certs = load_certificate(cert_path).await?;
    let private_key = load_keys(key_path).await?;

    let mut cfg = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, private_key)
        .expect("bad certificate/key");
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::from(cfg));
    Ok(tls_acceptor)
}
