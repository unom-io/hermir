//! HTTP behind a trait, so channels are tested against a fake and the CLI uses `ureq`.
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::progress::{Event, Progress};

pub trait Http {
    fn get_json(&self, url: &str) -> Result<serde_json::Value>;
    /// Downloads `url` to `part`, resuming from whatever `part` already holds. Returns the
    /// final size.
    fn download(&self, url: &str, part: &Path, progress: &dyn Progress) -> Result<u64>;
}

const USER_AGENT: &str = concat!(
    "hermir/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/unom-io/hermir)"
);

/// The real client. `GITHUB_TOKEN`, when set, lifts the API's anonymous rate limit.
pub struct Ureq {
    agent: ureq::Agent,
    github_token: Option<String>,
}

impl Ureq {
    pub fn new() -> Ureq {
        let agent = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .build()
            .into();
        Ureq {
            agent,
            github_token: std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()),
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

impl Http for Ureq {
    fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let mut req = self.agent.get(url).header("Accept", "application/json");
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

    fn download(&self, url: &str, part: &Path, progress: &dyn Progress) -> Result<u64> {
        let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        let mut req = self.agent.get(url);
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
        }
        let mut resp = req.call().map_err(|e| net(url, e))?;
        let status = resp.status().as_u16();
        let (append, mut done) = match status {
            206 => (true, have),
            200 => (false, 0),
            s => return Err(net(url, format!("HTTP {s}"))),
        };
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|len| len + if append { have } else { 0 });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(append)
            .write(true)
            .truncate(!append)
            .open(part)
            .map_err(|e| Error::io("open", part, e))?;
        let mut reader = resp.body_mut().as_reader();
        let mut buf = vec![0u8; 256 * 1024];
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
        Ok(done)
    }
}

/// Hex sha256 of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| Error::io("open", path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| Error::io("read", path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
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
        fn download(&self, url: &str, part: &Path, progress: &dyn Progress) -> Result<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_known_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
