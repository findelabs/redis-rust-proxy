// Findelabs Notes:
// https://ayende.com/blog/176577/first-run-with-rust-the-echo-server
// https://ayende.com/blog/176705/rust-based-load-balancing-proxy-server-with-async-i-o
// https://stevedonovan.github.io/rust-gentle-intro/7-shared-and-networking.html
// https://tutorialedge.net/rust/using-rwlocks-and-condvars-rust/
//
// This proxy that forwards data from a redis client to the redis server,
// with the master being found be asking the sentinel
//
// This proxy utilizes the Tokio runtime, which uses a thread pool to
// concurrently process each TCP connection along with all other active
// TCP connections across multiple threads, if configured
//
// You can compile this proxy with:
//
//     cargo build --release
//
// Then somthing like this in another terminal
//
//     ./target/release/tokio-proxy --master mymaster --listen 127.0.0.1:8080 --sentinel 127.0.0.1:26379
//
// You then should be able to run a benchmark like this on your local port 8080:
// 
// redis-benchmark -q -c 400 -n 10000 -p 8080

extern crate tokio;
extern crate getopts;
extern crate redis;
extern crate chrono;
extern crate net2;

use std::env;
use std::process::exit;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use tokio::runtime::{Builder};
use tokio::io::{copy, shutdown};
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;
use tokio::reactor;
use redis::{parse_redis_url};
use chrono::{DateTime, Utc};
use getopts::Options;
use net2::TcpBuilder;

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} --master master-name --listen ADDR --sentinel ADDR", program);
    print!("{}", opts.usage(&brief));
}

// 1. Get sentinel address and transform into proper connection string for redis
// 2. Establish connection, or show error
// 3. Create socket for sharing
fn get_current_master(addr: &SocketAddr, master: &String) -> Result<SocketAddr, redis::RedisError> {

    let sentinel_connection_format = format!("redis://{}", addr);
    let sentinel_connection_str : &str = &sentinel_connection_format[..];
    let sentinel_connection_url = parse_redis_url(&sentinel_connection_str).unwrap();

    // Establish connection, or return error
    let client = redis::Client::open(sentinel_connection_url)?;
    let mut con = client.get_connection()?;

    let current_master_info : Vec<String> = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(master).query(&mut con)
        .map_err(|e| println!("Failed to get current master from sentinel: {}", e)).unwrap();

    let current_master_socket = format!("{}:{}", &current_master_info[0], &current_master_info[1]);
    let current_master_socket = current_master_socket.parse::<SocketAddr>().expect("Failed to unwrap socket");

    Ok(current_master_socket)
}

// Ping the master, to ensure that it is up
// This should be used in the future if we end up load balancing between replicas
fn redis_master_health(addr: &SocketAddr) -> Result<String, redis::RedisError> {
    let master_connection_format = format!("redis://{}", addr);
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

// Simple get time function for better logging
fn get_time() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.to_rfc3339()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {

    let mut runtime = Builder::new()
        .blocking_threads(1000)
        .core_threads(1)
        .stack_size(3 * 1024 * 1024)
        .build()
        .unwrap();

    // Getops for proxy
    let args: Vec<String> = env::args().collect();          // Get script arguments
    let program = args[0].clone();                          // This is the script as it was called
    let mut opts = Options::new();

    // Flags for getopts
    opts.reqopt("m", "master", "name of master redis node", "MASTER");
    opts.reqopt("l", "listen", "local socket", "ADDR");
    opts.reqopt("s", "sentinel", "sentinel address", "ADDR");
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("d", "debug", "enable debugging");

    // Ensure that all flags are recognized
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => { m }
        Err(f) => { println!("{}",f.to_string()); print_usage(&program, opts); exit(2) }
    };
 
    // If -h is passed, then show usage
    if matches.opt_present("h") {
        print_usage(&program, opts);
        exit(1);
    };
  
    let enable_debug = match matches.opt_present("d") {
        true => { println!("{} - Enabling debugging", get_time()); true },
        false => false
    };
  
    // Get master name and sockets for listen and sentinel
    let user_listen_addr = matches.opt_str("l").unwrap();
    let user_sentinel_addr = matches.opt_str("s").unwrap();
    let user_master_name = matches.opt_str("m").unwrap();

    // Convert user provided vars to socket addresses
    let listen_addr = user_listen_addr.parse::<SocketAddr>()?;
    let sentinel_addr = user_sentinel_addr.to_socket_addrs().expect("Could not resolve sentinel address provided").next().unwrap();

    // Ensure that we are able to get the master address from the sentinel
    let master_addr = get_current_master(&sentinel_addr, &user_master_name);
    let master_addr = match master_addr {
        Ok(addr) => addr,
        Err(e) => { println!("Failed to connect to sentinel at {}: {}", &user_sentinel_addr, e); exit(2) }
    };

    // Create shared resource to pass the redis master's address with
    let resource = Arc::new(RwLock::new(master_addr));

    // Clone resource for each thread to use
    let resource_write = resource.clone();
    let resource_sentinel_read = resource.clone();
    let resource_health_read = resource.clone();
    let resource_main_read = resource.clone();

    // Create a socket with net2, so that we can reuse ports
    let net2_socket = TcpBuilder::new_v4().unwrap()
        .reuse_address(true).unwrap()
        .bind(listen_addr).unwrap()
        .listen(2048).unwrap();
    let socket = TcpListener::from_std(net2_socket, &reactor::Handle::default())?;

    // Announce were everything is at
    println!("{} - Startup Message: Listening on: {}", get_time(), listen_addr);
    println!("{} - Startup Message: Sentinel at: {}", get_time(), &user_sentinel_addr);
    println!("{} - Startup Message: Current master socket: {}", get_time(), &master_addr); 

    // Spawn loop to get master socket from sentinel
    let _get_current_master_socket = thread::spawn(move || {
        loop {

            // Ensure that we are able to get the master address from the sentinel
            let current_master_socket = get_current_master(&sentinel_addr, &user_master_name);
            let current_master_socket = match current_master_socket {
                Ok(addr) => addr,
                Err(e) => { 
                    println!("{} - Sentinel: Failed to connect to {}: {}", get_time(), &user_sentinel_addr, e); 
                    thread::sleep(Duration::from_millis(1000));
                    continue }
            };

            let known_master_socket = resource_sentinel_read
                .read()
                .unwrap()
                .to_string()
                .parse::<SocketAddr>()
                .unwrap();

            if current_master_socket != known_master_socket {
                println!("{} - Sentinel: Detected change in master", get_time());
                let mut resource_locked = resource_write.write().unwrap();
                println!("{} - Sentinel: Old master: {}", get_time(), &known_master_socket);
                *resource_locked = current_master_socket;
                println!("{} - Sentinel: New master: {}", get_time(), &current_master_socket);
                
                match redis_master_health(&current_master_socket) {
                    Ok(p) => println!("{} - Master Health: {} returned: {}", get_time(), current_master_socket, p),
                    Err(e) => println!("{} - Master Health: {} error: {}", get_time(), current_master_socket, e),
                };
            };
            
            thread::sleep(Duration::from_millis(1000));
        };
    });

    // Spawn healthcheck for redis master
    let _get_redis_master_health = thread::spawn(move || {
        loop {
            let known_master_socket = resource_health_read
                .read()
                .unwrap()
                .to_string()
                .parse::<SocketAddr>()
                .unwrap();

            match redis_master_health(&known_master_socket) {
                Ok(p) => { 
                    if enable_debug {
                        println!("{} - Master Health: {} returned: {}", get_time(), known_master_socket, p);
                    };
                    thread::sleep(Duration::from_millis(10000)) 
                },
                Err(e) => { println!("{} - Master Health: {} error: {}", get_time(), known_master_socket, e);
                    thread::sleep(Duration::from_millis(1000)) }
            };
        };
    });

    let done = socket
        .incoming()
        .map_err(|e| println!("{} - Primary accept loop: Error accepting client connection: {:?}", get_time(), e))
        .for_each(move |client| {

            // Get the master's current socket
            let current_master_socket = resource_main_read.read().unwrap();

            let server = TcpStream::connect(&current_master_socket);
            
            let amounts = server.and_then(move |server| {
                server.set_nodelay(true).unwrap();

                // Display something for the logs, warning, this is the main loop, so enabling it is expensive
                let peer = &server.local_addr().unwrap();
                let cache = &server.peer_addr().unwrap();

                if enable_debug {
                    println!("{} - Proxy: connecting {} to {}", get_time(), peer, cache);
                };

                // Create separate read/write handles for the TCP clients that we're
                // proxying data between. Note that typically you'd use
                // `AsyncRead::split` for this operation, but we want our writer
                // handles to have a custom implementation of `shutdown` which
                // actually calls `TcpStream::shutdown` to ensure that EOF is
                // transmitted properly across the proxied connection.
                //
                // As a result, we wrap up our client/server manually in arcs and
                // use the impls below on our custom `MyTcpStream` type.
                let client_reader = MyTcpStream(Arc::new(Mutex::new(client)));
                let client_writer = client_reader.clone();
                let server_reader = MyTcpStream(Arc::new(Mutex::new(server)));
                let server_writer = server_reader.clone();

                // Copy the data (in parallel) between the client and the server.
                // After the copy is done we indicate to the remote side that we've
                // finished by shutting down the connection.
                let client_to_server = copy(client_reader, server_writer)
                    .and_then(|(n, _, server_writer)| shutdown(server_writer).map(move |_| n));

                let server_to_client = copy(server_reader, client_writer)
                    .and_then(|(n, _, client_writer)| shutdown(client_writer).map(move |_| n));

                client_to_server.join(server_to_client)
            });

            // Possible errors here:
            // Client disconnected prematurely: Transport endpoint is not connected (os error 107)
            let msg = amounts.map(move|_x|{}).map_err(|e| { 
                println!("{} - Error connecting to redis: {}", get_time(), e);
            });

            runtime.spawn(msg);

            Ok(())
        });

    tokio::run(done);
    Ok(())
}

// This is a custom type used to have a custom implementation of the
// `AsyncWrite::shutdown` method which actually calls `TcpStream::shutdown` to
// notify the remote end that we're done writing.
#[derive(Clone)]
struct MyTcpStream(Arc<Mutex<TcpStream>>);

impl Read for MyTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.lock().unwrap().read(buf)
    }
}

impl Write for MyTcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AsyncRead for MyTcpStream {}

impl AsyncWrite for MyTcpStream {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        self.0.lock().unwrap().shutdown(Shutdown::Write)?;
        Ok(().into())
    }
}
