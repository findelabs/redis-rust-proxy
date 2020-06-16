# redis-rust-proxy

This is a simple rust network proxy for master-slave redis deployments. The rust proxy queries the sentinel to be aware of the master's location at all times, then proxies incoming connections to the current master cache. 

### Installation

Once rust has been [installed](https://www.rust-lang.org/tools/install), simply run:
```
cargo install --git https://github.com/findelabs/redis-rust-proxy.git
```

### Usage

The proxy requires three arguments for operation:

  - listen: Specify the listening socket
  - master: Specify the redis master name
  - sentinel: Specify the redis sentinel socket

There is an optional --debug flag also available, which shows a little more info, such as incoming connections.
