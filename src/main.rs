extern crate chrono;
extern crate fern;
extern crate log;
#[allow(unused_imports)]
use log::*;

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{self, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

struct Inner {
    delay: Duration,
    running: Arc<(Mutex<bool>, Condvar)>,
    pid: Option<u32>,
}

struct Runner {
    inner: Arc<Mutex<Inner>>,
    prog: String,
}

impl Runner {
    pub fn new(prog: &str) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                delay: Duration::from_secs(15),
                running: Arc::new((Mutex::new(false), Condvar::new())),
                pid: None,
            })),
            prog: prog.into(), // program to run.
        }
    }

    pub fn kill(pid: u32) {
        debug!("trying to kill: {}", pid);

        match Command::new("taskkill")
            .stdout(Stdio::null())
            .arg("/pid")
            .arg(pid.to_string())
            .arg("/f")
            .status()
        {
            Ok(status) => {
                debug!("killed: {} ({})", pid, status);
            }
            Err(err) => {
                warn!("couldn't kill: pid {}. {}", pid, err);
            }
        }
    }

    pub fn toggle(&self, status: bool) {
        let pair = {
            let inner = self.inner.clone();
            let inner = &*inner.lock().unwrap();
            inner.running.clone()
        };

        let &(ref lock, ref cv) = &*pair;
        let mut running = lock.lock().unwrap();
        *running = status;
        cv.notify_one();
    }

    pub fn delay(&self, parts: &[&str]) {
        let delay = match parts.get(1) {
            Some(param) => param.parse::<u64>().unwrap(),
            None => 15,
        };

        info!("!! got a delay command, setting delay to {}s", delay);
        let inner = self.inner.clone();
        let inner = &mut *inner.lock().unwrap();
        inner.delay = Duration::from_secs(delay);
    }

    pub fn restart(&self) {
        info!("!! got a restart command, restarting");
        let pid = {
            let inner = self.inner.clone();
            let inner = &*inner.lock().unwrap();
            inner.pid.unwrap()
        };
        Runner::kill(pid);
    }

    pub fn stop(&self) {
        info!("!! got a stop command, stopping");
        let pid = {
            let inner = self.inner.clone();
            let inner = &*inner.lock().unwrap();
            inner.pid.unwrap()
        };
        self.toggle(false);
        Runner::kill(pid);
    }

    pub fn start(&self) {
        info!("!! got a start command, starting");
        self.toggle(true);
    }

    pub fn handle(&self, cmd: &str) {
        let parts = cmd.split_whitespace().map(|s| s.trim()).collect::<Vec<_>>();
        match parts.get(0) {
            Some(&"STOP") => self.stop(),
            Some(&"START") => self.start(),
            Some(&"RESTART") => self.restart(),
            Some(&"DELAY") => self.delay(&parts),
            None => error!("invalid data"),
            _ => warn!("unknown command: '{}'", cmd),
        }
    }

    pub fn accept_connections(&self, socket: &TcpListener) {
        fn try_read<R: ::std::io::Read>(reader: R) -> Option<String> {
            let mut reader = BufReader::new(reader);
            let mut buf = vec![];
            reader.read_until(b'\0', &mut buf).ok()?;
            buf.pop(); // remove the NUL
            String::from_utf8(buf).ok()
        }

        for stream in socket.incoming() {
            match stream {
                Ok(stream) => match try_read(stream) {
                    Some(data) => self.handle(&data),
                    _ => error!("couldn't read data from stream"),
                },
                Err(err) => {
                    error!("failed to accept conn: {}", err);
                }
            }
        }
    }

    pub fn run_loop(&self) {
        self.toggle(true);
        loop {
            match Command::new(&self.prog)
                .stdout(process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let pid = {
                        let pid = child.id();
                        let inner = self.inner.clone();
                        let inner = &mut *inner.lock().unwrap();
                        inner.pid = Some(pid);
                        pid
                    };
                    info!("starting with pid: {}", pid);
                    match child.wait() {
                        Ok(status) => info!("{} exited: {}", self.prog, status),
                        Err(err) => warn!("could not start {}: {}", self.prog, err),
                    }
                }
                Err(err) => warn!("could not start child process: {}", err),
            };

            let delay = {
                let inner = self.inner.clone();
                let inner = &*inner.lock().unwrap();
                inner.delay
            };

            info!("?? waiting for {} to respawn", delay.as_secs());
            thread::sleep(delay);

            let pair = {
                let inner = self.inner.clone();
                let inner = &*inner.lock().unwrap();
                inner.running.clone()
            };
            let &(ref lock, ref cv) = &*pair;
            let mut running = lock.lock().unwrap();
            while !*running {
                running = cv.wait(running).unwrap();
            }
        }
    }
}

static COMMANDS: [&'static str; 4] = [
    "STOP          <- stops the bot",
    "START         <- starts the bot",
    "RESTART       <- restarts the bot",
    "DELAY <secs>  <- sets respawn duration to 'secs'",
];

fn main() {
    if let Err(err) = init_logger() {
        eprintln!("error! cannot start logger: {}", err);
        ::std::process::exit(1)
    }

    let prog = env::var("RUNNER_PROCESS").unwrap_or_else(|_| "noye.exe".into());
    let addr = env::var("RUNNER_ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());
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

    match TcpListener::bind((addr.as_str(), port)).ok() {
        Some(socket) => {
            let runner = Arc::new(Runner::new(&prog));
            {
                let runner = Arc::clone(&runner);
                info!("listening on {}:{}", addr, port);
                thread::spawn(move || loop {
                    runner.accept_connections(&socket);
                });
            }
            {
                let runner = Arc::clone(&runner);
                runner.run_loop()
            }
        }
        None => {
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

            if let Ok(mut client) = TcpStream::connect((addr.as_str(), port)) {
                let data = &args[1..].join(" ");
                if let Err(err) = client.write_fmt(format_args!("{}\0", data)) {
                    error!("couldn't send data: '{}' --> {}", data, err);
                    process::exit(-1);
                }
            } else {
                error!("couldn't connect to server at {}:{}", addr, port);
                process::exit(-1);
            }
        }
    }
}

fn init_logger() -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}] {}: {}",
                record.level(),
                chrono::Local::now().format("%T%.3f"),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .chain(fern::log_file("runner.log")?)
        .apply()?;
    Ok(())
}
