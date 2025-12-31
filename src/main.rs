use pingora::http::StatusCode;
use async_trait::async_trait;
use dashmap::DashMap;
use pingora::prelude::*;
use pingora::server::Server;
use pingora::upstreams::peer::HttpPeer;
use pingora_core::server::configuration::Opt;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{http_proxy_service, ProxyHttp, Session};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, RwLock};
use std::net::SocketAddr;
use redis::{AsyncCommands, Client, aio::ConnectionManager}; // Change Import

#[derive(Clone)]
struct Router {
    redis_conn: ConnectionManager,
    upstream_tls: bool,
}
struct HostRouter(Router);

impl HostRouter {
        pub async fn new(redis_url: &str, tls: bool) -> Result<Self, redis::RedisError> {
        // 1. Create the client
        let client = Client::open(redis_url)?;
        
        // 2. Get the async multiplexed connection (safe for sharing/cloning)
                                                               //
        let mut conn: ConnectionManager = ConnectionManager::new(client).await?;
        // 3. (Optional) Replicate your original hardcoded "insert" logic
        // We write these to Redis on startup so your existing logic is preserved.
        let initial_routes = vec![
            ("app1.localhost", "127.0.0.1:8001"),
            ("test.rustacean.shop", "10.163.36.230:8080"),
            ("test1.rustacean.shop", "127.0.0.1:8000"),
        ];

        for (host, addr) in initial_routes {
                    // SET key value
                    conn.set::<_, _, ()>(host, addr).await?;    
        }

        // 4. Return the struct
        Ok(Self(Router {
            redis_conn: conn,
            upstream_tls: tls,
        }))
    }


    // In impl HostRouter
    pub async fn resolve_host(&self, req: &RequestHeader) -> Result<SocketAddr, ResponseHeader> {
        // FIX 1: Use req.uri (field) instead of req.uri() (method)
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

        // ... rest of the Redis logic (same as before) ...
                let mut conn = self.0.redis_conn.clone();
        let addr_str: Option<String> = match conn.get(&host).await {
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
    }

    // Helper to generate error responses easily
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
    let certfolder = "/etc/letsencrypt/live/test.rustacean.shop";
    let cert_path = format!("{}/cert.pem", certfolder);
    let key_path = format!("{}/privkey.pem", certfolder);

    let opt = Opt::parse_args();

       let router = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            HostRouter::new("redis://127.0.0.1:6379", false).await
        }).expect("Failed to connect to Redis during startup")
    };



    let mut server = Server::new(Some(opt)).unwrap();
    server.bootstrap();


    let mut service = http_proxy_service(&server.configuration, router);
    service.add_tcp("0.0.0.0:80");
    server.add_service(service);

           let router = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            HostRouter::new("redis://127.0.0.1:6379", false).await
        }).expect("Failed to connect to Redis during startup")
    };
    let mut service = http_proxy_service(&server.configuration, router);
    service
        .add_tls("0.0.0.0:443", &cert_path, &key_path)
        .unwrap();
    println!("webservice is running on port: {}", "443");
    server.add_service(service);

    server.run_forever()
}
