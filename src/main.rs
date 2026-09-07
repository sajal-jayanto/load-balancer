use std::{sync::Arc, time::Duration};
use async_trait::async_trait;
use pingora::prelude::*;

pub struct LB(Arc<LoadBalancer<RoundRobin>>);

#[async_trait]
impl ProxyHttp for LB {
  type CTX = ();
  fn new_ctx(&self) {}

  async fn upstream_peer(&self, _s: &mut Session, _ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
    let upstream = self
      .0
      .select(b"", 256)
      .ok_or_else(|| Error::new_str("no healthy upstream"))?;

    Ok(Box::new(HttpPeer::new(upstream, false, String::new())))
  }
}

fn main() {
    env_logger::init();

    let mut server = Server::new(None).unwrap();
    server.bootstrap();

    let backend_instances = [
      "127.0.0.1:9001", 
      "127.0.0.1:9002"
    ];

    let mut upstreams: LoadBalancer<RoundRobin> = LoadBalancer::try_from_iter(backend_instances).unwrap();

    upstreams.set_health_check(TcpHealthCheck::new());
    upstreams.health_check_frequency = Some(Duration::from_secs(2));

    let hc = background_service("health check", upstreams);
    let upstreams = hc.task();

    let mut proxy = http_proxy_service(&server.configuration, LB(upstreams));
    proxy.add_tcp("0.0.0.0:8080");

    server.add_service(hc);
    server.add_service(proxy);
    server.run_forever();
}