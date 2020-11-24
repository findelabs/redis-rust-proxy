use futures::future::try_join;
use redis::parse_redis_url;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::state;

#[derive(Debug)]
pub enum RedisToolsError {
    //  RedisConError,
    MasterNoSlave,
}

impl std::error::Error for RedisToolsError {}

impl fmt::Display for RedisToolsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            //      RedisToolsError::RedisConError => write!(f, "Could not connect to cache"),
            RedisToolsError::MasterNoSlave => write!(f, "Master has no slave"),
        }
    }
}

// Find out if cache is the master, and return None if any errors are encountered
pub fn is_master(master_addr: &SocketAddr) -> Result<bool, Box<dyn Error>> {
    let master_connection_format = format!("redis://{}", master_addr);
    let master_connection_str: &str = &master_connection_format[..];
    let master_connection_url =
        parse_redis_url(&master_connection_str).expect("failed to parse redis url");

    let client = redis::Client::open(master_connection_url)?;
    let mut con = client.get_connection()?;

    let info: redis::InfoDict = redis::cmd("INFO").query(&mut con)?;
    let role: String = info.get("role").expect("Could not get role from info");

    match role.as_str() {
        "master" => Ok(true),
        _ => Ok(false),
    }
}

// Return connected slave to cache
pub fn get_slave(master_addr: &SocketAddr) -> Result<SocketAddr, Box<dyn Error>> {
    let master_connection_format = format!("redis://{}", master_addr);
    let master_connection_str: &str = &master_connection_format[..];
    let master_connection_url =
        parse_redis_url(&master_connection_str).expect("failed to parse redis url");

    let client = redis::Client::open(master_connection_url)?;
    let mut con = client.get_connection()?;

    let info: redis::InfoDict = redis::cmd("INFO").query(&mut con)?;
    let slave0_map = match info.get::<String>("slave0") {
        Some(v) => {
            let items: HashMap<String, String> = v
                .split(',')
                .map(|x| {
                    let vec: Vec<String> = x.split('=').map(|v| v.to_string()).collect();
                    (vec[0].to_string(), vec[1].to_string())
                })
                .collect();
            items
        }
        None => HashMap::new(),
    };

    let ip = slave0_map.get("ip");
    let port = slave0_map.get("port");

    match (ip, port) {
        (Some(ip), Some(port)) => {
            let socket = format!("{}:{}", ip, port);
            Ok(socket.parse::<SocketAddr>()?)
        }
        _ => Err(Box::new(RedisToolsError::MasterNoSlave)),
    }
}

// Get current master from the sentinel
pub fn get_current_master(
    sentinel_addr: &SocketAddr,
    master: &str,
    id: &str,
    sentinel_timeout: Duration,
) -> Result<SocketAddr, Box<dyn Error>> {
    let sentinel_connection_format = format!("redis://{}", sentinel_addr);
    let sentinel_connection_str: &str = &sentinel_connection_format[..];
    let sentinel_connection_url = parse_redis_url(&sentinel_connection_str).unwrap();

    // Establish connection, or return error
    let client = redis::Client::open(sentinel_connection_url)?;
    let mut con = client.get_connection_with_timeout(sentinel_timeout)?;

    let current_master_info: Vec<String> = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(master)
        .query(&mut con)
        .map_err(|e| {
            log::error!("{} - Failed to get current master from sentinel: {}", id, e);
        })
        .unwrap();

    let current_master_socket = format!("{}:{}", &current_master_info[0], &current_master_info[1]);
    let current_master_socket = current_master_socket.parse::<SocketAddr>()?;

    Ok(current_master_socket)
}

// Look for master from vector of discovered caches
pub fn find_master(discovered_masters: Vec<SocketAddr>, id: &str) -> Option<SocketAddr> {
    let masters_pretty: Vec<String> = discovered_masters.iter().map(|v| v.to_string()).collect();

    log::info!(
        "{} - Searching for the master amongst {}",
        id,
        &masters_pretty.join(", ")
    );
    for master in discovered_masters {
        match is_master(&master) {
            Ok(true) => {
                log::info!("{} - Found the master: {}", id, &master);
                return Some(master);
            }
            Ok(false) => {
                log::info!("{} - It appears that {} is not the master", id, &master);
                continue;
            }
            Err(e) => {
                log::error!("{} - {} error checking for master: {}", id, &master, e);
                continue;
            }
        };
    }
    None
}

//pub async fn transfer<'a>(mut inbound: TcpStream, resource: Arc<RwLock<SocketAddr>>, master: String, sentinel_addr: SocketAddr, id: String) -> Result<(), Box<dyn Error + Send>> {
pub async fn transfer(
    mut inbound: TcpStream,
    resource: state::State,
    id: String,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let resource_read = resource.inner.read().await;
    let known_master_socket = resource_read.last_known_master;

    // Get current master address, and update resource if socket has changed
    let current_master_addr = match get_current_master(
        &resource_read.sentinel_addr,
        &resource_read.master,
        &id,
        resource_read.sentinel_timeout,
    ) {
        Ok(socket) => socket,
        Err(e) => {
            log::error!("{} - Error getting current master from sentinel: {}", id, e);

            // First, check if the last known master is STILL the master. If it is not,
            // then go through all discovered masters to find the current master
            log::info!(
                "{} - Checking if {} is still the master",
                id,
                known_master_socket
            );
            match is_master(&known_master_socket) {
                Ok(true) => {
                    log::info!(
                        "{} - Looks like {} is still the master",
                        id,
                        &known_master_socket
                    );
                    known_master_socket
                }
                _ => {
                    log::info!(
                        "{} - It does not appear that {} is the master",
                        id,
                        &known_master_socket
                    );
                    match find_master(resource_read.discovered_masters.clone(), &id) {
                        Some(s) => s,
                        None => {
                            log::error!("{} - Could not locate the current master, defaulting to last master: {}", id, known_master_socket);
                            known_master_socket
                        }
                    }
                }
            }
        }
    };

    // Drop rwlock, to free up more connections
    drop(resource_read);

    // Update current master in State and check for new slaves
    if current_master_addr != known_master_socket {
        let mut resource_locked = resource.inner.write().await;

        // Update current master
        resource_locked.last_known_master = current_master_addr;

        // Update discovered_masters if vec does not already contain the addr
        match resource_locked.discovered_masters.contains(&current_master_addr) {
            true => {
                log::info!(
                    "{} - New master {} already in discovered caches",
                    id,
                    &current_master_addr.to_string()
                );
            },
            false => {
                log::info!(
                    "{} - Added new master {} to discovered caches",
                    id,
                    &current_master_addr.to_string()
                );
                resource_locked.discovered_masters.push(current_master_addr);
            }
        };

        // Check for any new connected slave
        if let Ok(slave) = get_slave(&current_master_addr) {
            if !resource_locked.discovered_masters.contains(&slave) {
                log::info!(
                    "{} - Added slave {} to discovered caches",
                    id,
                    &slave.to_string()
                );
                resource_locked.discovered_masters.push(slave);
            };
        };

        log::info!(
            "{} - Updated current master address to {}",
            id,
            &current_master_addr
        );
    };

    let mut outbound = TcpStream::connect(current_master_addr).await?;

    let (mut ri, mut wi) = inbound.split();
    let (mut ro, mut wo) = outbound.split();

    let client_to_server = async {
        io::copy(&mut ri, &mut wo).await?;
        wo.shutdown().await
    };

    let server_to_client = async {
        io::copy(&mut ro, &mut wi).await?;
        wi.shutdown().await
    };

    try_join(client_to_server, server_to_client).await?;

    Ok(())
}
