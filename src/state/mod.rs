use clap::ArgMatches;
use std::error::Error;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::redis_tools;

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct Inner {
    pub master: String,
    pub sentinel_addr: SocketAddr,
    pub sentinel_timeout: Duration,
    pub last_known_master: SocketAddr,
    pub discovered_masters: Vec<SocketAddr>,
    pub password: String
}

#[derive(Debug, Clone)]
pub struct State {
    pub inner: Arc<RwLock<Inner>>,
}

impl State {
    pub fn new(opts: ArgMatches) -> Result<Self, Box<dyn Error>> {
        // Get master name
        let master = opts.value_of("master").unwrap().to_string();

        // Get sentinel address, convert to socket
        let sentinel_addr_resolve = opts
            .value_of("sentinel")
            .unwrap()
            .to_string()
            .to_socket_addrs();

        // Get resolved sentinel address
        let sentinel_addr = match sentinel_addr_resolve {
            Ok(mut address) => address.next().unwrap(),
            Err(e) => {
                log::error!("Error resolving sentinel address: {}", e);
                exit(2)
            }
        };

        // Get sentinel timeout u64
        let sentinel_timeout_u64 = match opts
            .value_of("sentinel_timeout")
            .unwrap()
            .to_string()
            .parse::<u64>()
        {
            Ok(p) => {
                log::info!("Using a {}ms sentinel timeout", p);
                p
            }
            Err(e) => {
                log::error!("Error parsing sentinel_timeout: {}", e);
                exit(2)
            }
        };

        // Get sentinel timeout Duration
        let sentinel_timeout = Duration::from_millis(sentinel_timeout_u64);

        // Get password
        let password = opts
            .value_of("password")
            .unwrap()
            .to_string();

        // Get current master, and save into resource
        let current_master_addr = match redis_tools::get_current_master(
            &sentinel_addr,
            &master,
            &"start",
            sentinel_timeout,
            &password
        ) {
            Ok(socket) => {
                log::info!("Current master socket: {}", &socket);
                socket
            }
            Err(e) => {
                log::error!("Error connecting to sentinel: {}", e);
                exit(2)
            }
        };

        // Created discovered_masters, and add any connected slaves
        let discovered_masters = match redis_tools::get_slave(&current_master_addr, &password) {
            Ok(slave) => {
                log::info!("Discovered slave connected to master: {}", &slave);
                vec![current_master_addr, slave]
            }
            Err(_) => vec![current_master_addr],
        };

        // Create initial State
        let state = Inner {
            master,
            sentinel_addr,
            sentinel_timeout,
            last_known_master: current_master_addr,
            discovered_masters,
            password
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(state)),
        })
    }
}
