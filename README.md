# rust-redis-proxy

This is a simple rust proxy for master-slave redis deployments. The rust proxy queries the sentinel to be aware of the master's location at all times, then proxies incoming connections to the current master. 
