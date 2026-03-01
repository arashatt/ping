use async_trait::async_trait;
use dashmap::DashMap;
use pingora::http::StatusCode;
use pingora::prelude::*;
use pingora::server::Server;
use pingora::upstreams::peer::HttpPeer;
use pingora_core::server::configuration::Opt;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{http_proxy_service, ProxyHttp, Session};
use redis::{aio::ConnectionManager, AsyncCommands, Client}; // Change Import
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::sync::OnceCell;

#[derive(Clone)]
struct Router {
    redis_client: redis::Client,
    redis_conn: OnceCell<ConnectionManager>,
    upstream_tls: bool,
}

struct HostRouter(Router);
impl Router {
    async fn get_conn(&self) -> Result<ConnectionManager, redis::RedisError> {
        self.redis_conn
            .get_or_try_init(|| async { ConnectionManager::new(self.redis_client.clone()).await })
            .await
            .cloned()
    }
}
impl HostRouter {
    pub async fn new(redis_url: &str, tls: bool) -> Result<Self, redis::RedisError> {
        let client = Client::open(redis_url)?;

        // Clone before moving into ConnectionManager
        let mut conn: ConnectionManager = ConnectionManager::new(client.clone()).await?;

        let initial_routes = vec![
            ("app1.localhost", "127.0.0.1:8001"),
            ("test.rustacean.shop", "10.163.36.230:8080"),
            ("test1.rustacean.shop", "127.0.0.1:8000"),
        ];

        for (host, addr) in initial_routes {
            conn.set::<_, _, ()>(host, addr).await?;
        }

        Ok(Self(Router {
            redis_client: client,        // original client kept here
            redis_conn: OnceCell::new(), // lazy connection for actual requests
            upstream_tls: tls,
        }))
    }
    // In impl HostRouter
    pub async fn resolve_host(&self, req: &RequestHeader) -> Result<SocketAddr, ResponseHeader> {
        let host = if let Some(h) = req.uri.host() {
            h.to_string()
        } else if let Some(h_header) = req.headers.get("Host") {
            match h_header.to_str() {
                Ok(s) => s.split(':').next().unwrap_or(s).to_string(),
                Err(_) => return Err(self.make_error(StatusCode::BAD_REQUEST)),
            }
        } else {
            return Err(self.make_error(StatusCode::BAD_REQUEST));
        };

        let mut conn = match self.0.get_conn().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Redis connection error: {}", e);
                return Err(self.make_error(StatusCode::BAD_GATEWAY));
            }
        };

        let addr_str: Option<String> = match conn.get::<String, Option<String>>(host).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Redis error: {}", e);
                return Err(self.make_error(StatusCode::BAD_GATEWAY));
            }
        };

        match addr_str {
            Some(s) => match s.parse::<SocketAddr>() {
                Ok(addr) => Ok(addr),
                Err(_) => Err(self.make_error(StatusCode::BAD_GATEWAY)),
            },
            None => Err(self.make_error(StatusCode::NOT_FOUND)),
        }
    } // <-- resolve_host ends here

    // make_error is a separate method, not nested inside resolve_host
    fn make_error(&self, code: StatusCode) -> ResponseHeader {
        let mut resp = ResponseHeader::build(code, Some(0)).unwrap();
        resp.insert_header("Server", "Pingora-Proxy").unwrap();
        resp
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
    ) -> Result<Box<HttpPeer>> {
        // FIX 1: Call resolve_host directly on 'self'
        // Do not use self.Router.resolve_host
        match self.resolve_host(session.req_header()).await {
            Ok(addr) => {
                // FIX 2: Access the inner Router struct using 'self.0'
                // Do not use self.Router.0
                let tls = self.0.upstream_tls;

                let peer = Box::new(HttpPeer::new(addr, tls, "example.com".to_string()));
                Ok(peer)
            }
            Err(_resp) => {
                // Return a generic error
                // (You can also construct a specific pingora::Error here)
                Err(pingora::Error::new(pingora::ErrorType::HTTPStatus(502)).into())
            }
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let certfolder = ".";
    let cert_path = format!("{}/cert.pem", certfolder);
    let key_path = format!("{}/key.pem", certfolder);

    let opt = Opt::parse_args();

    let client = redis::Client::open("redis://127.0.0.1:6379")?;
    let router = HostRouter(Router {
        redis_client: client.clone(),
        redis_conn: OnceCell::new(), // initialized on first request inside Pingora's runtime
        upstream_tls: false,
    });

    let mut server = Server::new(Some(opt)).unwrap();
    server.bootstrap();

    let mut service = http_proxy_service(&server.configuration, router);
    service.add_tcp("0.0.0.0:80");
    server.add_service(service);
    let router_tls = HostRouter(Router {
        redis_client: client,
        redis_conn: OnceCell::new(),
        upstream_tls: false,
    });

    let mut service = http_proxy_service(&server.configuration, router_tls);
    service
        .add_tls("0.0.0.0:443", &cert_path, &key_path)
        .unwrap();
    println!("webservice is running on port: {}", "443");
    server.add_service(service);

    server.run_forever()
}
