//! Shared harness for the conformance suite: process management (spawn a
//! REAL gatewayd + mocks per test, kill on drop), a raw HTTP/1.1 client,
//! JWT minting, and the `target/conformance.json` writer.

use std::io::{Read, Write as IoWrite};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gateway_core::jwt;

// ------------------------------------------------------------- harness

/// Distinct ports per test; tests run in parallel.
static NEXT_PORT: AtomicU16 = AtomicU16::new(17100);

pub fn ports(n: u16) -> Vec<u16> {
    let base = NEXT_PORT.fetch_add(n, Ordering::SeqCst);
    (base..base + n).collect()
}

/// A child process killed on drop — no leaked gatewayds after the run.
pub struct Proc {
    child: Child,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn spike_fixture(name: &str) -> String {
    manifest_dir()
        .join("../../spikes/event-model/fixtures")
        .join(name)
        .display()
        .to_string()
}

pub fn demo_fixture(name: &str) -> String {
    manifest_dir().join("demo").join(name).display().to_string()
}

pub fn temp_file(tag: &str, content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gatewayd-conf-{}-{tag}-{}.yaml",
        std::process::id(),
        NEXT_PORT.fetch_add(1, Ordering::SeqCst),
    ));
    std::fs::write(&path, content).expect("write temp config");
    path
}

fn wait_for_port(port: u16, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{name} did not start listening on port {port}");
}

pub fn spawn_gatewayd(cfg_yaml: &str, port: u16, tag: &str) -> Proc {
    let cfg = temp_file(tag, cfg_yaml);
    let child = Command::new(env!("CARGO_BIN_EXE_gatewayd"))
        .args([
            "--config",
            &cfg.display().to_string(),
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--poll-interval",
            "0",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn gatewayd");
    let proc = Proc { child };
    wait_for_port(port, "gatewayd");
    proc
}

pub fn spawn_mock(port: u16, fixture: &str, provider: &str, require_sigv4: bool) -> Proc {
    let mut args = vec![
        "--port".to_string(),
        port.to_string(),
        "--fixture".to_string(),
        fixture.to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--delay-ms".to_string(),
        "5".to_string(),
    ];
    if require_sigv4 {
        args.push("--require-sigv4".to_string());
    }
    let child = Command::new(env!("CARGO_BIN_EXE_mock_upstream"))
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mock_upstream");
    let proc = Proc { child };
    wait_for_port(port, "mock_upstream");
    proc
}

pub fn spawn_sts(port: u16) -> Proc {
    let child = Command::new(env!("CARGO_BIN_EXE_mock_sts"))
        .args(["--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mock_sts");
    let proc = Proc { child };
    wait_for_port(port, "mock_sts");
    proc
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Raw HTTP/1.1 request over one connection, response read to EOF.
pub fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect gatewayd");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        req.push_str(&format!("{name}: {value}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body).unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Response {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let mut body = raw[head_end + 4..].to_vec();
    let chunked = headers
        .iter()
        .any(|(n, v)| n == "transfer-encoding" && v.eq_ignore_ascii_case("chunked"));
    if chunked {
        body = dechunk(&body);
    }
    Response { status, headers, body }
}

fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(pos) = rest.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&rest[..pos]).trim(),
            16,
        )
        .unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = pos + 2;
        out.extend_from_slice(&rest[start..start + size]);
        rest = &rest[start + size + 2..];
    }
    out
}

pub fn mint_jwt(claims: serde_json::Value) -> String {
    jwt::sign_hs256(
        claims.as_object().expect("claims object"),
        b"conformance-secret",
    )
}

// -------------------------------------------------- conformance.json

struct Entry {
    check: &'static str,
    test: &'static str,
    pass: bool,
}

static RESULTS: LazyLock<Mutex<Vec<Entry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn record(check: &'static str, test: &'static str, pass: bool) {
    let mut results = RESULTS.lock().expect("results lock");
    results.push(Entry { check, test, pass });
    let json = serde_json::json!({
        "suite": "gatewayd-phase1-conformance",
        "generated_unix": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "checks": results.iter().map(|e| serde_json::json!({
            "check": e.check,
            "test": e.test,
            "pass": e.pass,
        })).collect::<Vec<_>>(),
    });
    let target = manifest_dir().join("../../target");
    let _ = std::fs::create_dir_all(&target);
    let _ = std::fs::write(
        target.join("conformance.json"),
        serde_json::to_vec_pretty(&json).expect("serialize"),
    );
}

/// Run one named check: the verdict lands in conformance.json whether it
/// passes or panics; a failure still fails the test normally.
pub fn check(gb: &'static str, test: &'static str, f: impl FnOnce() + std::panic::UnwindSafe) {
    let result = std::panic::catch_unwind(f);
    record(gb, test, result.is_ok());
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
