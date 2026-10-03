//! HTTP behind a trait, so channels are tested against a fake and the CLI uses `ureq`.
//!
//! A download resumes from its `.part` only when the server shows the bytes continue the same
//! file: `If-Range` with the validator the part was started from, a `Content-Range` that starts
//! where the part ends, and a total that is still the size the channel said. Anything else
//! starts over, once.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::progress::{Event, Progress};

/// How hermir fetches release metadata and downloads: [`Ureq`] does it, a test answers instead.
pub trait Http: Send + Sync {
    /// The JSON document at `url`.
    fn get_json(&self, url: &str) -> Result<serde_json::Value>;
    /// Downloads `url` to `part`, resuming from whatever `part` already holds. `expected_size`,
    /// when the channel knows it, sets the time budget and is what the part must add up to.
    /// Returns the final size.
    fn download(
        &self,
        url: &str,
        part: &Path,
        expected_size: Option<u64>,
        progress: &dyn Progress,
    ) -> Result<u64>;
}

const USER_AGENT: &str = concat!(
    "hermir/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/unom-io/hermir)"
);

/// Each of resolving, connecting, sending the request and waiting for the response headers.
const STEP: Duration = Duration::from_secs(30);

/// The body's budget. ureq's is a total, not an idle timeout, so it grows with the size: ten
/// minutes plus the size at 50 KiB/s, or an hour when the size is unknown. A download that
/// runs out resumes on the next run.
pub(crate) fn body_budget(size: Option<u64>) -> Duration {
    match size {
        Some(n) => Duration::from_secs(600 + n / (50 * 1024)),
        None => Duration::from_secs(3600),
    }
}

/// The real client: https only, the OS's trust store (so a TLS-inspecting proxy or antivirus
/// the OS trusts works), a timeout on every step. `GITHUB_TOKEN`, when set, lifts the API's
/// anonymous rate limit.
pub struct Ureq {
    agent: ureq::Agent,
    github_token: Option<String>,
    /// A fixed body budget in place of [`body_budget`]; tests only.
    body: Option<Duration>,
}

fn config(https_only: bool) -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .http_status_as_error(false)
        .https_only(https_only)
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_resolve(Some(STEP))
        .timeout_connect(Some(STEP))
        .timeout_send_request(Some(STEP))
        .timeout_recv_response(Some(STEP))
}

impl Ureq {
    /// The client hermir uses unless told otherwise.
    pub fn new() -> Ureq {
        Ureq {
            agent: config(true).build().into(),
            github_token: std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()),
            body: None,
        }
    }

    /// Plain http, no proxy, short steps and a fixed body budget: for a server in a test.
    #[cfg(test)]
    pub(crate) fn local(body: Duration) -> Ureq {
        let step = Some(Duration::from_secs(2));
        Ureq {
            agent: config(false)
                .proxy(None)
                .timeout_connect(step)
                .timeout_recv_response(step)
                .build()
                .into(),
            github_token: None,
            body: Some(body),
        }
    }
}

impl Default for Ureq {
    fn default() -> Self {
        Ureq::new()
    }
}

fn net(op: &str, why: impl ToString) -> Error {
    Error::Network {
        op: op.into(),
        why: why.to_string(),
    }
}

/// What a part was started from, kept beside it until it is whole: the server's validator,
/// sent back as `If-Range`, and the total it announced.
#[derive(Default, Serialize, Deserialize)]
struct Started {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    if_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
}

fn started_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".started");
    PathBuf::from(s)
}

/// One request's outcome.
enum Fetched {
    Whole(u64),
    /// The part cannot be continued; why, should the second start fail too.
    Restart(String),
}

/// `bytes <start>-<end>/<total|*>` → the start and the total.
fn content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let (range, total) = v.trim().strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

impl Ureq {
    fn fetch(
        &self,
        url: &str,
        part: &Path,
        expected: Option<u64>,
        progress: &dyn Progress,
    ) -> Result<Fetched> {
        let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        let started: Started = match have {
            0 => Started::default(),
            _ => std::fs::read(started_path(part))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
        };
        // What the part must add up to: the channel's size, else what the server first said.
        let want = expected.or(started.total);
        if let Some(w) = want
            && have > w
        {
            return Ok(Fetched::Restart(format!(
                "the part holds {have} of {w} bytes"
            )));
        }
        let budget = self.body.unwrap_or_else(|| body_budget(expected));
        let mut req = self
            .agent
            .get(url)
            .config()
            .timeout_recv_body(Some(budget))
            .build()
            .header("Accept-Encoding", "identity");
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
            if let Some(v) = &started.if_range {
                req = req.header("If-Range", v);
            }
        }
        let mut resp = req.call().map_err(|e| net(url, e))?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let length = header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
        let range = header("content-range");
        let (offset, total) = match resp.status().as_u16() {
            206 => {
                let Some((start, total)) = range.as_deref().and_then(content_range) else {
                    return Ok(Fetched::Restart("a 206 without a Content-Range".into()));
                };
                if start != have {
                    return Ok(Fetched::Restart(format!(
                        "the server resumed at byte {start}; the part holds {have}"
                    )));
                }
                if let (Some(w), Some(t)) = (want, total)
                    && w != t
                {
                    return Ok(Fetched::Restart(format!(
                        "the file is {t} bytes now, not {w}"
                    )));
                }
                (have, total.or(want))
            }
            // From the start: a fresh part, a server that ignores ranges, or an `If-Range` that
            // no longer matches.
            200 => {
                if let (Some(e), Some(l)) = (expected, length)
                    && e != l
                {
                    return Err(net(
                        url,
                        format!("{l} bytes, not the {e} resolved; it changed since, run again"),
                    ));
                }
                let validator = header("etag")
                    .filter(|t| !t.starts_with("W/"))
                    .or_else(|| header("last-modified"));
                let s = Started {
                    if_range: validator,
                    total: length.or(expected),
                };
                let sp = started_path(part);
                let json = serde_json::to_vec(&s).expect("serializes");
                std::fs::write(&sp, json).map_err(|e| Error::io("write", &sp, e))?;
                (0, length.or(expected))
            }
            // Nothing past what the part holds: whole, when every size there is agrees.
            416 if have > 0 => {
                let server = range.as_deref().and_then(|v| {
                    v.trim()
                        .strip_prefix("bytes */")
                        .and_then(|t| t.trim().parse::<u64>().ok())
                });
                if let Some(total) = [want, server].into_iter().flatten().find(|t| *t != have) {
                    return Ok(Fetched::Restart(format!("416 at {have} of {total} bytes")));
                }
                progress.on(Event::Download {
                    done: have,
                    total: Some(have),
                });
                return Ok(Fetched::Whole(have));
            }
            s => return Err(net(url, format!("HTTP {s}"))),
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(offset > 0)
            .write(true)
            .truncate(offset == 0)
            .open(part)
            .map_err(|e| Error::io("open", part, e))?;
        let mut reader = resp.body_mut().as_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = offset;
        let mut last_report = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| net(url, e))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| Error::io("write", part, e))?;
            done += n as u64;
            if done - last_report >= 1 << 20 {
                progress.on(Event::Download { done, total });
                last_report = done;
            }
        }
        file.flush().map_err(|e| Error::io("write", part, e))?;
        progress.on(Event::Download { done, total });
        match total {
            Some(t) if done > t => Ok(Fetched::Restart(format!("{done} bytes of {t}"))),
            Some(t) if done < t => Err(net(url, format!("ended at byte {done} of {t}"))),
            _ => Ok(Fetched::Whole(done)),
        }
    }
}

impl Http for Ureq {
    fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let mut req = self
            .agent
            .get(url)
            .config()
            .timeout_recv_body(Some(self.body.unwrap_or(STEP)))
            .build()
            .header("Accept", "application/json");
        if let (Some(t), true) = (
            &self.github_token,
            url.starts_with("https://api.github.com/"),
        ) {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().map_err(|e| net(url, e))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().map_err(|e| net(url, e))?;
        if status != 200 {
            let hint = if status == 403 && url.contains("api.github.com") {
                " (rate limited? set GITHUB_TOKEN)"
            } else {
                ""
            };
            return Err(net(url, format!("HTTP {status}{hint}")));
        }
        serde_json::from_str(&body).map_err(|e| net(url, format!("not JSON: {e}")))
    }

    fn download(
        &self,
        url: &str,
        part: &Path,
        expected_size: Option<u64>,
        progress: &dyn Progress,
    ) -> Result<u64> {
        let started = started_path(part);
        let mut restarted = false;
        loop {
            match self.fetch(url, part, expected_size, progress)? {
                Fetched::Whole(n) => {
                    let _ = std::fs::remove_file(&started);
                    return Ok(n);
                }
                Fetched::Restart(why) => {
                    let _ = std::fs::remove_file(part);
                    let _ = std::fs::remove_file(&started);
                    if restarted {
                        return Err(net(url, format!("{why}, twice")));
                    }
                    restarted = true;
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::BTreeMap;

    /// Canned JSON per URL and canned bytes per download URL.
    #[derive(Default)]
    pub struct FakeHttp {
        pub json: BTreeMap<String, serde_json::Value>,
        pub files: BTreeMap<String, Vec<u8>>,
    }

    impl Http for FakeHttp {
        fn get_json(&self, url: &str) -> Result<serde_json::Value> {
            self.json
                .get(url)
                .cloned()
                .ok_or_else(|| net(url, "HTTP 404 (fake)"))
        }
        fn download(
            &self,
            url: &str,
            part: &Path,
            _expected_size: Option<u64>,
            progress: &dyn Progress,
        ) -> Result<u64> {
            let bytes = self
                .files
                .get(url)
                .ok_or_else(|| net(url, "HTTP 404 (fake)"))?;
            let have = std::fs::metadata(part)
                .map(|m| m.len() as usize)
                .unwrap_or(0);
            let rest = &bytes[have.min(bytes.len())..];
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(part)
                .map_err(|e| Error::io("open", part, e))?;
            f.write_all(rest).map_err(|e| Error::io("write", part, e))?;
            progress.on(Event::Download {
                done: bytes.len() as u64,
                total: Some(bytes.len() as u64),
            });
            Ok(bytes.len() as u64)
        }
    }
}

/// A release server on a loopback port, one thread per connection, no dependency: it answers
/// each request with what the test's handler says, and remembers what it was asked.
#[cfg(test)]
mod server {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// One request as the server saw it.
    #[derive(Clone, Debug, Default)]
    pub struct Seen {
        pub range: Option<String>,
        pub if_range: Option<String>,
    }

    impl Seen {
        /// The `N` of `Range: bytes=N-`.
        pub fn from(&self) -> Option<usize> {
            self.range
                .as_deref()?
                .strip_prefix("bytes=")?
                .strip_suffix('-')?
                .parse()
                .ok()
        }
    }

    pub enum Cut {
        /// Send this many bytes of the body, then hang up.
        Close(usize),
        /// Send this many bytes, then say nothing for a long while.
        Stall(usize),
    }

    pub struct Reply {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
        pub cut: Option<Cut>,
    }

    impl Reply {
        pub fn new(status: u16, body: &[u8]) -> Reply {
            Reply {
                status,
                headers: Vec::new(),
                body: body.to_vec(),
                cut: None,
            }
        }
        /// `206` with `body[from..]` of a file of `total` bytes.
        pub fn partial(body: &[u8], from: usize) -> Reply {
            let total = body.len();
            Reply::new(206, &body[from..]).header(
                "Content-Range",
                &format!("bytes {from}-{}/{total}", total - 1),
            )
        }
        pub fn header(mut self, k: &str, v: &str) -> Reply {
            self.headers.push((k.into(), v.into()));
            self
        }
        pub fn cut(mut self, cut: Cut) -> Reply {
            self.cut = Some(cut);
            self
        }
    }

    type Handler = dyn Fn(usize, &Seen) -> Reply + Send + Sync;

    pub struct Server {
        pub url: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        pub fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// Serves `handler(n, request)` for the `n`-th request, at `<url>/file`.
    pub fn serve(handler: impl Fn(usize, &Seen) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, handler) = (log.clone(), handler.clone());
                std::thread::spawn(move || answer(stream, &log, handler.as_ref()));
            }
        });
        Server { url, seen }
    }

    fn answer(mut stream: TcpStream, log: &Mutex<Vec<Seen>>, handler: &Handler) {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return,
            }
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        let header = |name: &str| {
            text.lines().skip(1).find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| v.trim().to_string())
            })
        };
        let req = Seen {
            range: header("range"),
            if_range: header("if-range"),
        };
        let n = {
            let mut log = log.lock().unwrap();
            log.push(req.clone());
            log.len() - 1
        };
        let reply = handler(n, &req);
        let mut out = format!(
            "HTTP/1.1 {} X\r\nConnection: close\r\nContent-Length: {}\r\n",
            reply.status,
            reply.body.len()
        );
        for (k, v) in &reply.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("\r\n");
        let _ = stream.write_all(out.as_bytes());
        match reply.cut {
            None => {
                let _ = stream.write_all(&reply.body);
            }
            Some(Cut::Close(n)) => {
                let _ = stream.write_all(&reply.body[..n]);
            }
            Some(Cut::Stall(n)) => {
                let _ = stream.write_all(&reply.body[..n]);
                let _ = stream.flush();
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::server::{Cut, Reply, serve};
    use super::*;
    use crate::progress::Quiet;
    use std::time::Instant;

    /// `n` bytes that differ per `seed`, so a mix of two files shows.
    fn file(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31) ^ seed).collect()
    }

    fn client() -> Ureq {
        Ureq::local(Duration::from_millis(300))
    }

    fn part() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.zip.part");
        (dir, p)
    }

    #[test]
    fn a_body_budget_grows_with_the_size() {
        assert_eq!(body_budget(None), Duration::from_secs(3600));
        assert_eq!(body_budget(Some(0)), Duration::from_secs(600));
        assert_eq!(
            body_budget(Some(100 * 1024 * 1024)),
            Duration::from_secs(600 + 2048)
        );
    }

    #[test]
    fn a_200_arrives_whole() {
        let body = file(1000, 1);
        let b = body.clone();
        let s = serve(move |_, _| Reply::new(200, &b).header("ETag", "\"a\""));
        let (_d, p) = part();
        assert_eq!(
            client().download(&s.url, &p, Some(1000), &Quiet).unwrap(),
            1000
        );
        assert_eq!(std::fs::read(&p).unwrap(), body);
        assert!(
            !started_path(&p).exists(),
            "nothing left beside a whole part"
        );
    }

    #[test]
    fn a_206_continues_the_part() {
        let body = file(1000, 2);
        let b = body.clone();
        let s = serve(move |_, r| match r.from() {
            Some(from) => Reply::partial(&b, from),
            None => Reply::new(200, &b),
        });
        let (_d, p) = part();
        std::fs::write(&p, &body[..400]).unwrap();
        assert_eq!(
            client().download(&s.url, &p, Some(1000), &Quiet).unwrap(),
            1000
        );
        assert_eq!(std::fs::read(&p).unwrap(), body);
        assert_eq!(s.seen()[0].range.as_deref(), Some("bytes=400-"));
    }

    #[test]
    fn a_416_is_whole_only_at_the_expected_size() {
        let body = file(1000, 3);
        let b = body.clone();
        let s = serve(move |_, r| match r.from() {
            Some(_) => Reply::new(416, b"").header("Content-Range", "bytes */1000"),
            None => Reply::new(200, &b),
        });
        // Whole: one request, nothing downloaded again.
        let (_d, p) = part();
        std::fs::write(&p, &body).unwrap();
        assert_eq!(
            client().download(&s.url, &p, Some(1000), &Quiet).unwrap(),
            1000
        );
        assert_eq!(s.seen().len(), 1);
        // Short: the part goes and the file comes again from the start.
        let (_d2, p2) = part();
        std::fs::write(&p2, &body[..600]).unwrap();
        assert_eq!(
            client().download(&s.url, &p2, Some(1000), &Quiet).unwrap(),
            1000
        );
        assert_eq!(std::fs::read(&p2).unwrap(), body);
        let seen = s.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2].range, None, "started over");
    }

    #[test]
    fn a_200_to_a_range_starts_over() {
        let body = file(1000, 4);
        let b = body.clone();
        let s = serve(move |_, _| Reply::new(200, &b));
        let (_d, p) = part();
        std::fs::write(&p, file(300, 99)).unwrap();
        assert_eq!(client().download(&s.url, &p, None, &Quiet).unwrap(), 1000);
        assert_eq!(std::fs::read(&p).unwrap(), body, "truncated, not appended");
    }

    #[test]
    fn a_206_elsewhere_than_the_part_ends_starts_over() {
        let body = file(1000, 5);
        let b = body.clone();
        let s = serve(move |_, r| match r.from() {
            Some(_) => Reply::partial(&b, 0),
            None => Reply::new(200, &b),
        });
        let (_d, p) = part();
        std::fs::write(&p, &body[..400]).unwrap();
        assert_eq!(
            client().download(&s.url, &p, Some(1000), &Quiet).unwrap(),
            1000
        );
        assert_eq!(std::fs::read(&p).unwrap(), body);
        assert_eq!(s.seen().len(), 2);
    }

    #[test]
    fn https_only_refuses_plain_http() {
        let s = serve(|_, _| Reply::new(200, b"{}"));
        let (_d, p) = part();
        let real = Ureq::new();
        let e = real.download(&s.url, &p, None, &Quiet).unwrap_err();
        assert!(matches!(e, Error::Network { .. }));
        assert!(e.to_string().contains("https only"), "{e}");
        assert!(real.get_json(&s.url).is_err());
        assert!(s.seen().is_empty(), "refused before connecting");
        assert!(!p.exists());
    }

    #[test]
    fn a_stalled_body_times_out_then_resumes() {
        let body = file(1000, 6);
        let b = body.clone();
        let s = serve(move |n, r| match (n, r.from()) {
            (0, _) => Reply::new(200, &b)
                .header("ETag", "\"v1\"")
                .cut(Cut::Stall(400)),
            (_, Some(from)) if r.if_range.as_deref() == Some("\"v1\"") => Reply::partial(&b, from),
            _ => Reply::new(200, &b),
        });
        let (_d, p) = part();
        let t = Instant::now();
        let e = client()
            .download(&s.url, &p, Some(1000), &Quiet)
            .unwrap_err();
        assert!(matches!(e, Error::Network { .. }), "{e}");
        assert!(e.to_string().contains("timeout"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 400);
        assert_eq!(
            client().download(&s.url, &p, Some(1000), &Quiet).unwrap(),
            1000
        );
        assert_eq!(std::fs::read(&p).unwrap(), body);
        assert_eq!(s.seen()[1].range.as_deref(), Some("bytes=400-"));
    }

    /// The release is replaced between two runs: `If-Range` no longer matches, and the server
    /// sends the new file whole.
    #[test]
    fn a_replaced_file_never_mixes_with_the_part() {
        let (a, b) = (file(1000, 7), file(1200, 8));
        let (a2, b2) = (a.clone(), b.clone());
        let s = serve(move |n, r| match n {
            0 => Reply::new(200, &a2)
                .header("ETag", "\"a\"")
                .cut(Cut::Close(500)),
            _ => match (r.from(), r.if_range.as_deref()) {
                (Some(from), Some("\"b\"")) => Reply::partial(&b2, from),
                _ => Reply::new(200, &b2).header("ETag", "\"b\""),
            },
        });
        let (_d, p) = part();
        assert!(client().download(&s.url, &p, None, &Quiet).is_err());
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 500);
        assert_eq!(client().download(&s.url, &p, None, &Quiet).unwrap(), 1200);
        assert_eq!(std::fs::read(&p).unwrap(), b);
        assert_eq!(s.seen()[1].if_range.as_deref(), Some("\"a\""));
        let _ = a;
    }

    /// The same, from a server that ignores `If-Range`: the total in `Content-Range` is not the
    /// one the part was started from, so it starts over.
    #[test]
    fn a_content_range_of_another_size_starts_over() {
        let (a, b) = (file(1000, 9), file(1200, 10));
        let (a2, b2) = (a.clone(), b.clone());
        let s = serve(move |n, r| match (n, r.from()) {
            (0, _) => Reply::new(200, &a2).cut(Cut::Close(500)),
            (_, Some(from)) => Reply::partial(&b2, from),
            (_, None) => Reply::new(200, &b2),
        });
        let (_d, p) = part();
        assert!(client().download(&s.url, &p, None, &Quiet).is_err());
        assert_eq!(client().download(&s.url, &p, None, &Quiet).unwrap(), 1200);
        assert_eq!(std::fs::read(&p).unwrap(), b);
        let seen = s.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[1].range.as_deref(), Some("bytes=500-"));
        assert_eq!(seen[2].range, None);
        let _ = a;
    }

    /// The channel said 1000 bytes and the server now has 1200: resolve again, do not install
    /// what nobody resolved.
    #[test]
    fn a_size_other_than_resolved_is_refused() {
        let b = file(1200, 11);
        let s = serve(move |_, r| match r.from() {
            Some(from) => Reply::partial(&b, from),
            None => Reply::new(200, &b),
        });
        let (_d, p) = part();
        std::fs::write(&p, file(500, 12)).unwrap();
        let e = client()
            .download(&s.url, &p, Some(1000), &Quiet)
            .unwrap_err();
        assert!(e.to_string().contains("not the 1000 resolved"), "{e}");
        assert!(!p.exists(), "the stale part is gone");
    }
}
