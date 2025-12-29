use async_trait::async_trait;
use pingora::prelude::*;
use pingora::server::Server;
use pingora::upstreams::peer::HttpPeer;
use pingora_core::server::configuration::Opt;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{http_proxy_service, ProxyHttp, Session};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tikv_client::{RawClient, Key, Value, Result as TikvResult};
use dashmap::DashMap;

#[derive(Clone)]
struct Router {
    routes: DashMap<String, SocketAddr>,
    tikv: Arc<RawClient>,
    upstream_tls: bool,
}
struct HostRouter(Arc<Router>);

impl HostRouter {
    async fn new(tls: bool, pd_endpoints: Vec<&str>) -> TikvResult<Self> {
        let mut routes = DashMap::new();
        let tikv = RawClient::new(pd_endpoints).await?;
        routes.insert(
            "app1.localhost".to_string(),
            "127.0.0.1:8001".parse().unwrap(),
        );
        routes.insert(
            "test.rustacean.shop".to_string(),
            "10.163.36.230:8080".parse().unwrap(),
        );
        routes.insert(
            "test1.rustacean.shop".to_string(),
            "127.0.0.1:8000".parse().unwrap(),
        );
        Ok (Self {
            0: Arc::new(Router {
                routes,
                tikv: Arc::new(tikv),
                upstream_tls: tls,
            }),
        })
    } 

        /// Load a single domain mapping from TiKV into memory
    pub async fn load_from_tikv(&self, domain: &str) -> TikvResult<Option<String>> {
        let key = Key::from(domain);
        if let Some(val) = self.tikv.get(key).await? {
            let upstream = String::from_utf8(val.into())
                .map_err(|e| tikv_client::Error::Other(e.to_string().into()))?;
            self.store.insert(domain.to_string(), upstream.clone());
            Ok(Some(upstream))
        } else {
            Ok(None)
        }
    }

    /// Update mapping in TiKV and memory
    pub async fn update_mapping(&self, domain: &str, upstream: &str) -> TikvResult<()> {
        self.tikv.put(Key::from(domain), Value::from(upstream)).await?;
        self.store.insert(domain.to_string(), upstream.to_string());
        Ok(())
    }

    /// Lookup from memory (fast path)
    pub fn get_upstream(&self, domain: &str) -> Option<String> {
        self.store.get(domain).map(|v| v.value().clone())
    }




    fn resolve_host(&self, req: &RequestHeader) -> Result<SocketAddr, ResponseHeader> {
        println!("req uri host {:?}", req.headers.get("host"));

        if let Some(host) = req.headers.get("host") {
            let s = String::from_utf8(host.as_bytes().to_vec()).unwrap();
            let host_only = s.split(':').next().unwrap_or(&s);
            println!("Host Only {:?}", host_only);
            if let Some(addr) = self.0.routes.get(host_only) {
                return Ok(*addr);
            } else {
                let mut hdr = ResponseHeader::build(404, None).unwrap();
                hdr.insert_header("Content-Type", "text/plain").unwrap();
                hdr.insert_header("Content-Length", "0").unwrap();
                return Err(hdr);
            }
        }

        let mut hdr = ResponseHeader::build(400, None).unwrap();
        hdr.insert_header("Content-Type", "text/plain").unwrap();
        hdr.insert_header("Content-Length", "0").unwrap();
        Err(hdr)
    }
}

#[async_trait]
impl ProxyHttp for HostRouter {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {
        ()
    }
    async fn upstream_peer(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>, Box<pingora::Error>> {
        println!("{:?}", self.0.upstream_tls);
        let req = session.req_header();
        println!("{:?}", req);
        match self.resolve_host(req) {
            Ok(addr) => {
                let peer = HttpPeer::new(addr, false, "".to_string());
                Ok(Box::new(peer))
            }
            Err(_resp_hdr) => Err(pingora::Error::explain(
                ErrorType::InternalError,
                "resp_hdr",
            )),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {

    let pd_endpoints = vec![
        "http://127.0.0.1:2379",
        "http://127.0.0.1:2380",
        "http://127.0.0.1:2381",
    ];

    let router = Router::new(pd_endpoints).await?;
    
    // Example usage
    router.update_mapping("example.com", "127.0.0.1:8080").await?;




    let certfolder = "/etc/letsencrypt/live/test.rustacean.shop";
    let cert_path = format!("{}/cert.pem", certfolder);
    let key_path = format!("{}/privkey.pem", certfolder);

    let opt = Opt::parse_args();

    let mut my_server = Server::new(Some(opt)).unwrap();
    my_server.bootstrap();

    let mut server = Server::new(None)?;
    server.bootstrap();

    let router = HostRouter::new(false, router);
    let mut service = http_proxy_service(&my_server.configuration, router);
    service.add_tcp("0.0.0.0:80");
    server.add_service(service);

    let router = HostRouter::new(true, router);
    let mut service = http_proxy_service(&my_server.configuration, router);
    service
        .add_tls("0.0.0.0:443", &cert_path, &key_path)
        .unwrap();
    println!("webservice is running on port: {}", "443");
    server.add_service(service);

    server.run_forever()
}
