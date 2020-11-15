use tokio::net::TcpListener;
use clap::{crate_version, App, Arg};
use net2::TcpBuilder;
use futures::FutureExt;
use std::env;
use std::sync::Arc;
use std::error::Error;
use std::net::SocketAddr;
use tokio::sync::RwLock;
use env_logger::{Builder, Target};
use log::LevelFilter;
use chrono::Local;
use std::io::Write;
use std::process::exit;
use std::iter;
use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;
use std::net::ToSocketAddrs;

mod redis_tools;

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct Inner {
    master: String,
    sentinel_addr: SocketAddr,
    last_known_master: SocketAddr,
    discovered_masters: Vec<SocketAddr>
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

    let listen_addr = opts.value_of("listen").unwrap().to_string();
    let master = opts.value_of("master").unwrap().to_string();

    let sentinel_addr_resolve = opts.value_of("sentinel")
                        .unwrap()
                        .to_string()
                        .to_socket_addrs();

    let sentinel_addr = match sentinel_addr_resolve {
        Ok(mut address) => {
            address.next().unwrap()
        },
        Err(e) => {
            log::error!("Error resolving sentinel address: {}", e);
            exit(2)
        }
    };
            
//                        .expect("Could not resolve sentinel address provided")
//                        .next()
//                        .unwrap();


    // Get current master, and save into resource
    let current_master_addr = match redis_tools::get_current_master(&sentinel_addr, &master, &"start") {
        Ok(socket) => { 
            log::info!("Current master socket: {}", &socket);
            socket
        },
        Err(e) => { 
            log::error!("Error connecting to sentinel: {}", e);
            exit(2)
        }
    };

    // Create initial State
    let state = Inner {
        master: master.clone(),
        sentinel_addr: sentinel_addr,
        last_known_master: current_master_addr,
        discovered_masters: vec![current_master_addr]
    };

    // Create shared resource in order to safely pass the current master socket
    let resource = State {
        inner: Arc::new(RwLock::new(state))
    };

    let net2_socket = TcpBuilder::new_v4().unwrap()
        .reuse_address(true).unwrap()
        .bind(&listen_addr).unwrap()
        .listen(2048).unwrap();

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
            if let Err(e) = r {
                log::info!("{} - Connection failed; {}", id, e);
            }
        });
    
        tokio::spawn(transfer);
    }

    Ok(())
}


