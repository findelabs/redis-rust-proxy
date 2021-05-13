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
use tokio::net::TcpListener;
use std::time::Duration;
use std::process::exit;

mod redis_tools;
mod state;

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
                .value_name("master")
                .help("the sentinel's name for the cache")
                .env("MASTER_NAME")
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
                .env("LISTEN_ADDR")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("sentinel")
                .short("s")
                .long("sentinel")
                .help("sentinel address")
                .required(true)
                .env("SENTINEL_ADDR")
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
        .arg(
            Arg::with_name("password")
                .short("p")
                .long("password")
                .help("use redis auth")
                .required(false)
                .default_value("")
                .env("REDIS_PASSWORD")
                .takes_value(true),
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

    let net2_socket = TcpBuilder::new_v4()
        .unwrap()
        .reuse_address(true)
        .unwrap()
        .bind(&listen_addr)
        .unwrap()
        .listen(2048)
        .unwrap();

    let mut listener = TcpListener::from_std(net2_socket)?;

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

    // Create state
    let resource = state::State::new(opts, sentinel_timeout)?;

    log::info!("Listening on: {}", listen_addr);

    while let Ok((inbound, client)) = listener.accept().await {
        // Create client id
        let mut rng = thread_rng();
        let id: String = iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .take(24)
            .collect();

        log::info!("{} - New connection from {}", &id, &client);

        let transfer = redis_tools::transfer(inbound, resource.clone(), id.clone(), sentinel_timeout).map(move |r| {
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
