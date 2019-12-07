# rust-redis-proxy

This is a simple rust proxy for master-slave redis deployments. The rust proxy queries the sentinel to be aware of the master's location at all times, then proxies incoming connections to the current master. 

### Installation

Once rust has been [installed](https://www.rust-lang.org/tools/install), simply run:
```
cargo install --git https://github.com/findelabs/rust-redis-proxy.git
```

