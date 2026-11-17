//! Runs the real binary, the way an operator starts it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_git-archiver");

fn write_config(data_dir: &Path) {
    std::fs::create_dir_all(data_dir).unwrap();
    std::fs::write(
        data_dir.join("config.toml"),
        "listen = \"127.0.0.1:0\"\nrepos = [\"https://github.com/foo/bar\"]\n",
    )
    .unwrap();
}

/// The real binary, with everything it writes captured as it goes.
struct Running {
    child: Child,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
    readers: Vec<std::thread::JoinHandle<()>>,
}

impl Running {
    /// Starts the binary with colours off and its output captured.
    fn start(mut command: Command) -> Running {
        let mut child = command
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (stdout, out_reader) = capture(child.stdout.take().unwrap());
        let (stderr, err_reader) = capture(child.stderr.take().unwrap());
        Running {
            child,
            stdout,
            stderr,
            readers: vec![out_reader, err_reader],
        }
    }

    /// What the app did so far: exit status and everything it wrote.
    fn report(&mut self) -> String {
        let status = match self.child.try_wait().unwrap() {
            Some(status) => format!("exited with {status}"),
            None => "still running".to_string(),
        };
        // An exited app closed its pipes, so its readers finish with the tail.
        if status.starts_with("exited") {
            for reader in self.readers.drain(..) {
                let _ = reader.join();
            }
        }
        format!(
            "app {status}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout.lock().unwrap(),
            self.stderr.lock().unwrap()
        )
    }

    /// Waits for the app's startup log line and returns the address it bound.
    fn addr(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let found = self
                .stdout
                .lock()
                .unwrap()
                .split_inclusive('\n')
                .find_map(|line| {
                    let line = line.strip_suffix('\n')?;
                    let rest = line.split("listening addr=").nth(1)?;
                    Some(rest.split_whitespace().next()?.to_string())
                });
            if let Some(addr) = found {
                return addr;
            }
            if self.child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                panic!("app never reported its address: {}", self.report());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Fetches the list page from the app, failing with what the app did.
    fn list_page(&mut self) -> String {
        let addr = self.addr();
        match fetch(&addr) {
            Ok(response) => response,
            Err(error) => panic!(
                "couldn't fetch the list page from {addr}: {error}\n{}",
                self.report()
            ),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Collects a pipe on a background thread so the app never blocks writing.
fn capture(
    mut pipe: impl Read + Send + 'static,
) -> (Arc<Mutex<String>>, std::thread::JoinHandle<()>) {
    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&buffer);
    let reader = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    });
    (buffer, reader)
}

fn fetch(addr: &str) -> std::io::Result<String> {
    let addr = addr.parse().unwrap();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

#[test]
fn the_data_directory_can_be_given_by_flag() {
    let dir = TempDir::new().unwrap();
    write_config(dir.path());

    let mut command = Command::new(BINARY);
    command
        .arg("--data-dir")
        .arg(dir.path())
        .env_remove("GIT_ARCHIVER_DATA_DIR");
    let mut app = Running::start(command);

    let page = app.list_page();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("https://github.com/foo/bar"), "{page}");
}

#[test]
fn the_data_directory_can_be_given_by_environment_variable() {
    let dir = TempDir::new().unwrap();
    write_config(dir.path());

    let mut command = Command::new(BINARY);
    command.env("GIT_ARCHIVER_DATA_DIR", dir.path());
    let mut app = Running::start(command);

    let page = app.list_page();
    assert!(page.contains("https://github.com/foo/bar"), "{page}");
}

#[test]
fn a_missing_data_directory_is_created_with_a_default_config() {
    let parent = TempDir::new().unwrap();
    let data_dir = parent.path().join("data");

    // The default config listens on port 8080, which may be taken; only the
    // written config matters here.
    let mut command = Command::new(BINARY);
    command.arg("--data-dir").arg(&data_dir);
    let app = Running::start(command);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !data_dir.join("config.toml").exists() {
        assert!(Instant::now() < deadline, "no config written");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(app);

    let config = std::fs::read_to_string(data_dir.join("config.toml")).unwrap();
    assert!(config.contains("interval = \"24h\""), "{config}");
}

#[test]
fn the_data_directory_is_required() {
    let output = Command::new(BINARY)
        .env_remove("GIT_ARCHIVER_DATA_DIR")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--data-dir"), "{stderr}");
}

#[test]
fn an_invalid_config_exits_with_a_clear_error() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), "interval = \"often\"\n").unwrap();

    let output = Command::new(BINARY)
        .arg("--data-dir")
        .arg(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid config"), "{stderr}");
    assert!(stderr.contains("config.toml"), "{stderr}");
    assert!(stderr.contains("`interval` must be a duration"), "{stderr}");
}
