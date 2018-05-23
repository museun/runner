use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{self, Command};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use notify::{self, Watcher};

use winapi::um::consoleapi;
use winapi::um::wincon;

struct Inner {
    delay: Duration,
    running: Arc<(Mutex<bool>, Condvar)>,
    pid: Option<u32>,
}

pub struct Runner {
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

    pub fn run_loop(&self) {
        use std::os::windows::process::CommandExt;

        // this has to be done here so watcher stays in scope.
        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::watcher(tx, Duration::from_secs(3)).expect("cannot create watcher");
        debug!("trying to watch for: {}", &self.prog);
        watcher
            .watch(&self.prog, notify::RecursiveMode::NonRecursive)
            .expect("cannot watch file");
        trace!("watching registered");

        self.toggle(true); // enable the blocking cvar
        loop {
            trace!("spawning process");
            match Command::new(&self.prog)
                .stdout(process::Stdio::null()) // disable processes stdout
                .creation_flags(0x00000200)     // CREATE_NEW_PROCESS_GROUP
                .spawn()
            {
                Ok(mut child) => {
                    let pid = {
                        let pid = child.id();
                        self.safely(move |inner| inner.pid = Some(pid));
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

            info!("?? waiting for event");
            self.wait_for_event(&rx);

            let pair = self.safely(move |inner| inner.running.clone());
            let &(ref lock, ref cv) = &*pair;
            let mut running = lock.lock().unwrap();
            while !*running {
                running = cv.wait(running).unwrap();
            }

            trace!("next loop");
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
                Err(err) => error!("failed to accept conn: {}", err),
            }
        }
    }

    fn delay(&self, parts: &[&str]) {
        let delay = match parts.get(1) {
            Some(param) => param.parse::<u64>().unwrap(),
            None => 15, // default to 15 secs
        };

        info!("!! got a delay command, setting delay to {}s", delay);
        self.safely(move |inner| inner.delay = Duration::from_secs(delay));
    }

    fn restart(&self) {
        info!("!! got a restart command, restarting");
        let pid = self.safely(move |inner| inner.pid.unwrap());
        Runner::kill(pid);
    }

    fn stop(&self) {
        info!("!! got a stop command, stopping");
        let pid = self.safely(move |inner| inner.pid.unwrap());
        self.toggle(false);
        Runner::kill(pid);
    }

    fn start(&self) {
        info!("!! got a start command, starting");
        self.toggle(true);
    }

    fn toggle(&self, status: bool) {
        let pair = self.safely(move |inner| inner.running.clone());
        let &(ref lock, ref cv) = &*pair;
        let mut running = lock.lock().unwrap();
        *running = status;
        cv.notify_one();
    }

    fn handle(&self, cmd: &str) {
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

    fn wait_for_event(&self, rx: &mpsc::Receiver<notify::DebouncedEvent>) {
        let delay = self.safely(move |inner| inner.delay);

        while match rx.recv_timeout(delay) {
            Ok(notify::DebouncedEvent::Write(_)) => {
                info!("{} was replaced. starting it", self.prog);
                false
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                info!(
                    "waited {} seconds, starting old {}.",
                    delay.as_secs(),
                    self.prog
                );
                false
            }
            Err(mpsc::RecvTimeoutError::Disconnected) | _ => true,
        } {}
    }

    // is this a bad idea?
    fn safely<T, F: Fn(&mut Inner) -> T + 'static>(&self, f: F) -> T {
        let inner = self.inner.clone();
        let inner = &mut *inner.lock().unwrap();
        f(inner)
    }

    fn kill(pid: u32) {
        debug!("trying to kill: {}", pid);
        info!("killing pid: {}", pid);
        unsafe {
            // remove our parent console
            wincon::FreeConsole();
            if wincon::AttachConsole(pid) > 0 {
                // detach it from self
                consoleapi::SetConsoleCtrlHandler(None, 1);
                // send CTRL_BREAK_EVENT to process group
                wincon::GenerateConsoleCtrlEvent(wincon::CTRL_BREAK_EVENT, pid);
            }
        }
        debug!("killed process");
    }
}
