//! Runs the real binary, the way an operator starts it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_git-archiver");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_config(data_dir: &Path, port: u16) {
    std::fs::create_dir_all(data_dir).unwrap();
    std::fs::write(
        data_dir.join("config.toml"),
        format!("listen = \"127.0.0.1:{port}\"\nrepos = [\"https://github.com/foo/bar\"]\n"),
    )
    .unwrap();
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for the app to answer on `port` and returns the list page.
fn list_page(app: &mut Running, port: u16) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            return response;
        }
        if let Some(status) = app.0.try_wait().unwrap() {
            panic!("app exited with {status}");
        }
        assert!(
            Instant::now() < deadline,
            "app didn't listen on port {port}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_data_directory_can_be_given_by_flag() {
    let dir = TempDir::new().unwrap();
    let port = free_port();
    write_config(dir.path(), port);

    let mut app = Running(
        Command::new(BINARY)
            .arg("--data-dir")
            .arg(dir.path())
            .env_remove("GIT_ARCHIVER_DATA_DIR")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let page = list_page(&mut app, port);
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("https://github.com/foo/bar"), "{page}");
}

#[test]
fn the_data_directory_can_be_given_by_environment_variable() {
    let dir = TempDir::new().unwrap();
    let port = free_port();
    write_config(dir.path(), port);

    let mut app = Running(
        Command::new(BINARY)
            .env("GIT_ARCHIVER_DATA_DIR", dir.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let page = list_page(&mut app, port);
    assert!(page.contains("https://github.com/foo/bar"), "{page}");
}

#[test]
fn a_missing_data_directory_is_created_with_a_default_config() {
    let parent = TempDir::new().unwrap();
    let data_dir = parent.path().join("data");

    // The default config listens on port 8080, which may be taken; only the
    // written config matters here.
    let mut app = Running(
        Command::new(BINARY)
            .arg("--data-dir")
            .arg(&data_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !data_dir.join("config.toml").exists() {
        assert!(Instant::now() < deadline, "no config written");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = app.0.kill();

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
