extern crate basiclogger;
extern crate log;
use log::*;

use std::env;
use std::time::Duration;
use std::thread;
use std::process::{self, Command, Stdio};
use std::net::{TcpListener, TcpStream};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Condvar, Mutex};

// static ADDRESS: &'static str = "127.0.0.1";
// static PORT: u16 = 54564;
// static PROCESS: &'static str = "foo.exe";

struct Inner {
    delay: Duration,
    running: Arc<(Mutex<bool>, Condvar)>,
    pid: Option<u32>,
}

struct Runner {
    inner: Arc<Mutex<Inner>>,
    conf: Arc<Config>,
}

impl Runner {
    pub fn new(conf: &Arc<Config>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                delay: Duration::from_secs(15),
                running: Arc::new((Mutex::new(false), Condvar::new())),
                pid: None,
            })),
            conf: Arc::clone(conf),
        }
    }

    pub fn kill(pid: u32) {
        debug!("trying to kill: {}", pid);

        match Command::new("taskkill")
            .stdout(Stdio::null())
            .arg("/pid")
            .arg(format!("{}", pid))
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

    pub fn delay(&self, parts: Vec<&str>) {
        let delay = match parts.get(1) {
            Some(param) => param.parse::<u64>().unwrap(),
            None => 15,
        };

        info!("!! got a delay command, setting delay: {}s", delay);
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

    pub fn handle(&self, cmd: String) {
        let parts = cmd.split_whitespace().map(|s| s.trim()).collect::<Vec<_>>();
        match parts.get(0) {
            Some(&"STOP") => self.stop(),
            Some(&"START") => self.start(),
            Some(&"RESTART") => self.restart(),
            Some(&"DELAY") => self.delay(parts),
            None => error!("invalid data"),
            _ => warn!("unknown command: '{}'", cmd),
        }
    }

    pub fn accept_connections(&self, socket: &TcpListener) {
        for stream in socket.incoming() {
            match stream {
                Ok(stream) => {
                    let mut reader = BufReader::new(stream);
                    let mut buf = vec![];
                    reader
                        .read_until(b'\0', &mut buf)
                        .expect("couldn't read command");
                    buf.pop();
                    self.handle(String::from_utf8(buf).expect("couldn't parse response as string"));
                }
                Err(err) => {
                    error!("failed to accept conn: {}", err);
                }
            }
        }
    }

    pub fn run_loop(&self) {
        use std::fs;

        self.toggle(true);
        loop {
            let file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .write(true)
                .open("noye.log")
                .expect("should be able to create log file");

            match Command::new(&self.conf.process).stdout(file).spawn() {
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
                        Ok(status) => info!("{} exited: {}", self.conf.process, status),
                        Err(err) => warn!("could not start {}: {}", self.conf.process, err),
                    }
                }
                Err(err) => warn!("could not start child process: {}", err),
            };

            let delay = {
                let inner = self.inner.clone();
                let inner = &*inner.lock().unwrap();
                inner.delay
            };

            info!("?? waiting delay: {}", delay.as_secs());
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

#[derive(Debug)]
struct Config {
    process: String,
    address: String,
    port: u16,
}

impl Config {
    pub fn new() -> Self {
        let get_or = |s, d| env::var(s).unwrap_or(d);

        Self {
            process: get_or("RUNNER_PROCESS", "noye.exe".into()),
            address: get_or("RUNNER_ADDRESS", "127.0.0.1".into()),
            port: get_or("RUNNER_PORT", "54145".into())
                .parse::<u16>()
                .unwrap(),
        }
    }
}

fn main() {
    use basiclogger::*;
    let _ = MultiLogger::init(
        vec![
            // TODO fix file logging
            Box::new(StdoutLogger::new()), // log to console
        ],
        Level::Trace,
    );

    let args = env::args()
        .map(|ref b| b.to_uppercase())
        .collect::<Vec<_>>();

    let conf = Arc::new(Config::new());
    let runner = Arc::new(Runner::new(&conf));
    match create_lock(&conf.address, conf.port) {
        Some(socket) => {
            {
                let runner = Arc::clone(&runner);
                info!("listening on {}:{}", conf.address, conf.port);
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
            if args.len() < 2 {
                println!("available commands:");
                for key in COMMANDS.iter().map(|s| s.to_lowercase()) {
                    println!(" - {}", key);
                }
                process::exit(-1);
            }
            send(&args[1..].join(" "), &conf.address, conf.port);
        }
    }
}

fn create_lock(addr: &str, port: u16) -> Option<TcpListener> {
    match TcpListener::bind((addr, port)) {
        Ok(socket) => Some(socket),
        Err(_) => None,
    }
}

fn send(data: &str, addr: &str, port: u16) {
    let data = format!("{}\0", data);
    let mut client = TcpStream::connect((addr, port))
        .expect(&format!("couldn't connect to server at {}:{}", addr, port,));
    client
        .write_all(data.as_bytes())
        .expect(&format!("couldn't send data: '{}'", data));
}
