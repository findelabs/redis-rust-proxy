use chrono::Local;
use clap::{crate_version, App, Arg};
use env_logger::{Builder, Target};
use futures::FutureExt;
use log::LevelFilter;
use net2::TcpBuilder;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use std::env;
use std::error::Error;
use std::io::Write;
use std::iter;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

mod redis_tools;

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct Inner {
    master: String,
    sentinel_addr: SocketAddr,
    sentinel_timeout: Duration,
    last_known_master: SocketAddr,
    discovered_masters: Vec<SocketAddr>,
}

#[derive(Debug, Clone)]
pub struct State {
    pub inner: Arc<RwLock<Inner>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let opts = App::new("redis-proxy")
        .version(crate_version!())
        .author("Daniel F. <dan@findelabs.com>")
        .about(
            "Simple proxy to forward redis clients to the current master, as located via sentinels",
        )
        .arg(
            Arg::with_name("master")
                .short("m")
                .long("master")
                .required(true)
                .value_name("MASTER")
                .help("the sentinel's name for the cache")
                .env("MASTER")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("listen")
                .short("l")
                .long("listen")
                .value_name("socket")
                .help("listening socket")
                .required(false)
                .default_value("0.0.0.0:6379")
                .env("LISTEN")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("sentinel")
                .short("s")
                .long("sentinel")
                .help("sentinel address")
                .required(true)
                .env("SENTINEL")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("sentinel_timeout")
                .short("t")
                .long("sentinel_timeout")
                .help("sentinel connection timeout")
                .required(false)
                .default_value("10")
                .env("SENTINEL_TIMEOUT")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("debug")
                .short("d")
                .long("debug")
                .help("enable debugging")
                .required(false)
                .takes_value(false),
        )
        .get_matches();

    // Initialize log Builder
    Builder::new()
        .format(|buf, record| {
            writeln!(
                buf,
                "{{\"date\": \"{}\", \"level\": \"{}\", \"message\": \"{}\"}}",
                Local::now().format("%Y-%m-%dT%H:%M:%S:%f"),
                record.level(),
                record.args()
            )
        })
        .target(Target::Stdout)
        .filter(None, LevelFilter::Info)
        .init();

    // Get listen address and master name
    let listen_addr = opts.value_of("listen").unwrap().to_string();
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

    // Get current master, and save into resource
    let current_master_addr = match redis_tools::get_current_master(
        &sentinel_addr,
        &master,
        &"start",
        sentinel_timeout,
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
    let discovered_masters = match redis_tools::get_slave(&current_master_addr) {
        Ok(slave) => {
            log::info!("Discovered slave connected to master: {}", &slave);
            vec![current_master_addr, slave]
        },
        Err(_) => vec![current_master_addr]
    };

    // Create initial State
    let state = Inner {
        master: master.clone(),
        sentinel_addr,
        sentinel_timeout,
        last_known_master: current_master_addr,
        discovered_masters
    };

    // Create shared resource in order to safely pass the current master socket
    let resource = State {
        inner: Arc::new(RwLock::new(state)),
    };

    let net2_socket = TcpBuilder::new_v4()
        .unwrap()
        .reuse_address(true)
        .unwrap()
        .bind(&listen_addr)
        .unwrap()
        .listen(2048)
        .unwrap();

    let mut listener = TcpListener::from_std(net2_socket)?;
    log::info!("Listening on: {}", listen_addr);

    while let Ok((inbound, client)) = listener.accept().await {
        let mut rng = thread_rng();
        let id: String = iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .take(24)
            .collect();

        log::info!("{} - New connection from {}", &id, &client);

        let transfer = redis_tools::transfer(inbound, resource.clone(), id.clone()).map(move |r| {
            match r {
                Ok(_) => {
                    log::info!("{} - Connection completed", id);
                }
                Err(e) => {
                    log::info!("{} - Connection failed: {}", id, e);
                }
            };
        });

        tokio::spawn(transfer);
    }

    Ok(())
}
