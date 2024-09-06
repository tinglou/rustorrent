use async_trait::async_trait;
use kv_log_macro::{debug, info};
use serde::de::DeserializeOwned;
use std::{convert::TryFrom, iter::FromIterator, net::SocketAddr, time::Duration};
use tokio::net::TcpStream;
use url::Url;

use std::sync::Arc;

use crate::{torrent::Result, tracker::http::announce_http, utils::ConnectTimeout};

use tokio_rustls::{
    rustls::{self, pki_types::ServerName},
    TlsConnector,
};

use super::{
    connection::TrackerConnection,
    http::{format_request, send_recv, AnnounceQuery, ToQuery},
    supervisor::TrackerData,
};

async fn https_get<R, Q>(tls: &TlsConnector, url: &Url, query: &Q, addr: &SocketAddr) -> Result<R>
where
    Q: ToQuery,
    R: DeserializeOwned,
{
    info!(
        "[https tracker]", {
            url: url.to_string(),
            host: url.host().map(|h| h.to_string()),
            port: url.port(),
            scheme: url.scheme()
        }
    );

    let port = url.port().unwrap_or(443);
    let addr = SocketAddr::from((addr.ip(), port));

    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).await?;
    let server_name = ServerName::try_from(url.domain().unwrap().to_string()).unwrap();
    let stream = tls.connect(server_name, stream).await?;

    let req = format_request(url, query);

    debug!("[https tracker] ", { request: req });

    send_recv(stream, &req).await
}

pub struct HttpsConnection {
    data: Arc<TrackerData>,
    addr: Vec<Arc<SocketAddr>>,
    tls: TlsConnector,
}

#[async_trait]
impl TrackerConnection for HttpsConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(self.data.as_ref());

        announce_http(&self.addr, connected_addr, |addr| {
            https_get(&self.tls, &self.data.url, &query, addr)
        })
        .await
    }

    async fn scrape(&mut self) -> Result<()> {
        Ok(())
    }
}

impl HttpsConnection {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        data: Arc<TrackerData>,
        addr: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync> {
        let root_store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let rc_config = Arc::new(config);

        let tls = TlsConnector::from(rc_config);

        Box::new(Self { data, addr, tls })
    }
}
