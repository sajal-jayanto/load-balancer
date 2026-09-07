# lb

A minimal HTTP load balancer built on [Pingora](https://github.com/cloudflare/pingora).

Round-robin across a static list of backends, with TCP health checks and
graceful binary upgrades.

## Requirements

- Rust 1.85+ (Pingora MSRV)
- Linux (tier 1; macOS mostly works, some features missing)
- `clang` and `perl 5` if you enable the TLS features

## Setup

```bash
cargo build --release
```

## Run

```bash
RUST_LOG=info cargo run
```

Listens on `0.0.0.0:8080`, proxies to `127.0.0.1:9001` and `127.0.0.1:9002`.

### Test with dummy backends

```bash
mkdir -p a b && echo A > a/index.html && echo B > b/index.html
(cd a && python3 -m http.server 9001) &
(cd b && python3 -m http.server 9002) &

for i in $(seq 4); do curl -s localhost:8080/; done   # A B A B
```

Kill one backend. Within ~2s it drops out of rotation and all traffic goes to
the survivor — no 502s. Restart it and it comes back.

## Architecture

```
Server                    process: config, CLI, daemonize, signals, graceful reload
 ├─ proxy service         TCP listener + ProxyHttp impl
 └─ background service    health checker driving the LoadBalancer
```

`LoadBalancer<RoundRobin>` is handed to `background_service()`, which takes
ownership and runs the probe loop. `hc.task()` returns an
`Arc<LoadBalancer<RoundRobin>>` — the same object — which the proxy reads from
in `upstream_peer()`. One object, two owners: the background loop writes health
status, the proxy reads it. No lock in application code.

### Request lifecycle

Hooks on the `ProxyHttp` trait, in order. Only `upstream_peer` is required.

| Hook | Purpose |
| --- | --- |
| `request_filter` | early reject, auth, short-circuit response |
| `upstream_peer` | **required** — pick a backend, return an `HttpPeer` |
| `upstream_request_filter` | rewrite headers going to the backend |
| `response_filter` | rewrite headers coming back |
| `logging` | access log; runs even on failure |

Per-request state goes in `type CTX`.

## Configuration

Backends and the listen address are hardcoded in `src/main.rs`.

Process-level settings come from a YAML file. To enable it, change
`Server::new(None)` to `Server::new(Some(Opt::parse_args()))`, then:

```yaml
# conf.yaml
version: 1
threads: 4
pid_file: /tmp/lb.pid
error_log: /tmp/lb_err.log
upgrade_sock: /tmp/lb.sock
```

```bash
RUST_LOG=info cargo run -- -c conf.yaml -d    # -d daemonizes
cargo run -- -h                               # full flag list
```

## Logging

Pingora logs through the `log` facade. Without a registered sink nothing prints,
so `main()` calls `env_logger::init()`. Level comes from `RUST_LOG`:

```bash
RUST_LOG=info                             # everything
RUST_LOG=warn,pingora_core=debug          # mixed
```

Default with `RUST_LOG` unset is `error` only. When daemonized, stderr is
redirected to the `error_log` path in `conf.yaml`.

## Binding port 80

Ports under 1024 need `CAP_NET_BIND_SERVICE`. Without it, `bind()` fails with
`EACCES` and the service thread panics at startup.

```bash
# option 1: capability on the binary (re-run after every rebuild)
sudo setcap 'cap_net_bind_service=+ep' target/release/lb

# option 2: kernel redirect, app stays on 8080
sudo iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080
```

For production, let systemd grant it:

```ini
[Service]
ExecStart=/usr/local/bin/lb -c /etc/lb/conf.yaml
AmbientCapabilities=CAP_NET_BIND_SERVICE
User=lb
Group=lb
Restart=on-failure
```

Avoid `sudo cargo run` — it leaves `target/` and the cargo registry root-owned
and your normal builds start failing.

## Graceful upgrade

Linux only. The old process hands its listening socket to the new one over
`upgrade_sock`, then drains in-flight requests. From a client's perspective the
listener never closes.

```bash
pkill -SIGQUIT lb && RUST_LOG=info ./target/release/lb -c conf.yaml -d -u
```

`SIGTERM` alone is a graceful shutdown: stop accepting, finish what's in flight,
exit.

## Health checks

`TcpHealthCheck` only proves the port accepts connections — an app can be
deadlocked and still pass. For a real check:

```rust
let mut hc = HttpHealthCheck::new("localhost", false); // (host, use_tls)
hc.req.set_uri("/health".parse().unwrap());
hc.consecutive_success = 2;   // probes to mark UP
hc.consecutive_failure = 3;   // probes to mark DOWN
upstreams.set_health_check(Box::new(hc));
```

Asymmetric thresholds are deliberate: slow to trust, fast to evict. Setting both
to 1 makes a flapping backend flap your traffic with it.

## Extending

- **Selection algorithm** — swap the generic parameter. `RoundRobin`,
  `LeastConnections`, `Random`, `Consistent` (ketama). Nothing else changes.
- **TLS termination** — `proxy.add_tls(addr, cert_path, key_path)`, and add the
  `openssl` or `boringssl` feature.
- **HTTPS upstreams** — `HttpPeer::new(addr, true, "sni.example.com".into())`.
- **Path routing** — match on `session.req_header().uri.path()` in
  `upstream_peer` and select from a different pool.
- **Forwarded headers** — implement `upstream_request_filter` and insert
  `X-Forwarded-For` from `session.client_addr()`.

## License

MIT