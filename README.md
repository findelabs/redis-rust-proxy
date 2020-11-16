# Redis Proxy

This is a simple network proxy for use in master-slave-sentinel redis deployments. The rust proxy queries the sentinel for each incoming client, then proxies incoming connections to the current master cache. The proxy also keeps track of known caches, to assist the proxy when the sentinel is unavailable. 

### Installation

Once rust has been [installed](https://www.rust-lang.org/tools/install), simply run:
```
cargo install --git https://github.com/findelabs/redis-rust-proxy.git
```

### Arguments

```
# Required
--master: 
    Specify the redis master name, env MASTER.
--sentinel: 
    Specify the redis sentinel socket, env SENTINEL.

# Optional
--listen: 
    Specify the listening socket for the proxy, env LISTEN. Default 0.0.0.0:6379
--sentinel_timeout: 
    Change the sentinel timeout, env SENTINEL_TIMEOUT. Default 10 (ms)
```

### Testing

You can deploy a test cluster with a master, slave, sentinel, and proxy, with the docker-compose.yml file under examples/. Creation of the cluster is as simple as:
```
# Install docker-compose
sudo wget -O /usr/local/bin/docker-compose https://github.com/docker/compose/releases/download/1.27.4/docker-compose-Linux-x86_64 && sudo chmod +x /usr/local/bin/docker-compose

# Run docker-compose
sudo docker-compose -f examples/docker-compose.yml up --build --remove-orphans
```
