use async_trait::async_trait;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use pingora::prelude::*;
use pingora::server::Server;
use pingora::upstreams::peer::HttpPeer;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session, http_proxy_service};
use pingora_core::server::configuration::Opt;

#[derive(Clone)]
struct HR {
    routes: HashMap<String, SocketAddr>,
}
struct HostRouter(Arc<HR>);

impl HostRouter {
    fn new() -> Self {
        let mut routes = HashMap::new();
        routes.insert("app1.localhost".to_string(), "127.0.0.1:8001".parse().unwrap());
        routes.insert("app2.localhost".to_string(), "127.0.0.1:8002".parse().unwrap());
        Self { 0: Arc::new(HR {routes})}
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
                let req = session.req_header();
                println!("{:?}",req);
        match self.resolve_host(req) {
            Ok(addr) => {
                let peer = HttpPeer::new(addr, false, "".to_string());
                Ok(Box::new(peer))
            }
            Err(_resp_hdr) =>     
            
                Err(pingora::Error::explain(ErrorType::InternalError,"resp_hdr" ))
        
                    }
    }
}

fn main() -> Result<(),Box<dyn std::error::Error>> {
        let opt = Opt::parse_args();
    let mut my_server = Server::new(Some(opt)).unwrap();
    my_server.bootstrap();


    let mut server = Server::new(None)?;
    server.bootstrap();

    let router = HostRouter::new();
    let mut service = http_proxy_service( &my_server.configuration, router);
    service.add_tcp("0.0.0.0:8084");
    println!("webservice is running on port: port: {}", "8084");

    server.add_service(service);
    server.run_forever()
}

