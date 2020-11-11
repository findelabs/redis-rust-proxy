use std::net::SocketAddr;
use redis::{parse_redis_url};
use tokio::net::TcpStream;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::io::AsyncWriteExt;
use tokio::io;
use futures::future::try_join;
use std::error::Error;
use std::process::exit;


pub fn get_redis_master_health(master_addr: &SocketAddr) -> Result<String, redis::RedisError> {
    let master_connection_format = format!("redis://{}", master_addr);
    let master_connection_str : &str = &master_connection_format[..];
    let master_connection_url = parse_redis_url(&master_connection_str).unwrap();

    let client = redis::Client::open(master_connection_url).unwrap();
    let mut con = client.get_connection()?;
    let healthcheck = redis::cmd("PING").query(&mut con);

    match healthcheck {
        Ok(p) => return Ok(p),
        Err(e) => return Err(e)
    };
}

pub fn get_current_master(sentinel_addr: &SocketAddr, master: &str, id: &str) -> Result<SocketAddr, Box<dyn Error>> {

    let sentinel_connection_format = format!("redis://{}", sentinel_addr);
    let sentinel_connection_str : &str = &sentinel_connection_format[..];
    let sentinel_connection_url = parse_redis_url(&sentinel_connection_str).unwrap();

    // Establish connection, or return error
    let client = redis::Client::open(sentinel_connection_url)?;
    let mut con = client.get_connection()?;

    let current_master_info : Vec<String> = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(master).query(&mut con)
        .map_err(|e| {
            log::info!("{} - Failed to get current master from sentinel: {}", id, e);
         }).unwrap();

    let current_master_socket = format!("{}:{}", &current_master_info[0], &current_master_info[1]);
    let current_master_socket = current_master_socket.parse::<SocketAddr>()?;

    Ok(current_master_socket)
}

pub async fn transfer<'a>(mut inbound: TcpStream, resource: Arc<RwLock<SocketAddr>>, master: String, sentinel_addr: SocketAddr, id: String) -> Result<(), Box<dyn Error>> {

    let known_master_socket = resource
        .read()
        .unwrap()
        .to_string()
        .parse::<SocketAddr>()
        .unwrap();

    // Get current master address, and update resource if socket has changed
    let current_master_addr = match get_current_master(&sentinel_addr, &master, &id) {
        Ok(socket) => { 
            if socket != known_master_socket {
                let mut resource_locked = match resource.write() {
                    Ok(r) => r,
                    Err(e) => {
                        log::info!("{} - Failed unlocking shared resourcing: {}", id, e);
                        log::info!("Exiting...");
                        exit(2)
                    }
                };
                *resource_locked = socket;
                log::info!("{} - Updated current master address to {}", id, &socket);
            };
            socket
        },
        Err(e) => { 
            log::info!("{} - Error getting current master from sentinel: {}", id, e);
            log::info!("{} - Using last known good master socket: {}", id, known_master_socket);
            known_master_socket
        }
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

