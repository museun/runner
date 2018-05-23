#[macro_use]
extern crate log;
extern crate env_logger;

use std::env;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::process;
use std::sync::Arc;
use std::thread;

extern crate runner;
use runner::Runner;

static COMMANDS: [&'static str; 4] = [
    "STOP          <- stops the bot",
    "START         <- starts the bot",
    "RESTART       <- restarts the bot",
    "DELAY <secs>  <- sets respawn duration to 'secs'",
];

fn main() {
    env::set_var(
        "RUNNER_LOG",
        env::var("RUNNER_LOG").unwrap_or_else(|_| "info".into()),
    );
    env_logger::Builder::from_env("RUNNER_LOG")
        .default_format_module_path(false)
        .init();

    const ADDRESS: &str = "127.0.0.1"; // no point in configuring this, localhost is the only sane address
    let prog = env::var("RUNNER_PROCESS").unwrap_or_else(|_| "noye.exe".into());
    let port = match env::var("RUNNER_PORT")
        .unwrap_or_else(|_| "54145".into())
        .parse::<u16>()
    {
        Ok(port) => port,
        Err(err) => {
            error!("couldn't parse the port. check RUNNER_PORT: {}", err);
            process::exit(1)
        }
    };

    trace!("before listener");
    debug!("attempting to bind to {}:{}", ADDRESS, port);
    match TcpListener::bind((ADDRESS, port)).ok() {
        Some(socket) => {
            let runner = Arc::new(Runner::new(&prog));
            {
                let runner = Arc::clone(&runner);
                info!("listening on {}:{}", ADDRESS, port);
                thread::spawn(move || loop {
                    runner.accept_connections(&socket);
                });
            }
            {
                let runner = Arc::clone(&runner);
                runner.run_loop();
                trace!("after run loop");
            }
        }
        None => {
            debug!("already running");
            let args = env::args()
                .map(|ref b| b.to_uppercase())
                .collect::<Vec<_>>();

            if args.len() < 2 {
                println!("available commands:");
                for key in COMMANDS.iter().map(|s| s.to_lowercase()) {
                    println!(" - {}", key);
                }
                process::exit(-1);
            }

            if let Ok(mut client) = TcpStream::connect((ADDRESS, port)) {
                let data = &args[1..].join(" ");
                if let Err(err) = client.write_fmt(format_args!("{}\0", data)) {
                    error!("couldn't send data: '{}' --> {}", data, err);
                    process::exit(-1);
                }
            } else {
                error!("couldn't connect to server at {}:{}", ADDRESS, port);
                process::exit(-1);
            }
        }
    }

    trace!("end of main");
}
