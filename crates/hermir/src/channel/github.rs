//! The latest GitHub release of a repository, one asset of it. Digests come from the API
//! (`sha256:<hex>` on assets uploaded since 2025-06); rolling tags (`latest`, `continuous`,
//! `build-<sha>`) carry no version, so the release id is the asset's: its upload time, id and
//! digest.
use crate::channel::http::Http;
use crate::error::{Error, Result};
use crate::model::{AssetFilter, Resolved};

pub fn resolve(
    http: &dyn Http,
    emulator: &str,
    repo: &str,
    filter: &AssetFilter,
) -> Result<Resolved> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let rel = http.get_json(&url)?;
    let tag = rel["tag_name"].as_str().unwrap_or("").to_string();
    let date = rel["published_at"]
        .as_str()
        .map(|s| s.chars().take(10).collect::<String>())
        .unwrap_or_default();
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = assets.iter().filter_map(|a| a["name"].as_str()).collect();
    let hits: Vec<&serde_json::Value> = assets
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| filter.matches(n) && !is_sidecar(n))
        })
        .collect();
    let asset = match hits.as_slice() {
        [one] => *one,
        [] => {
            return Err(Error::Catalog {
                entry: emulator.into(),
                why: format!(
                    "no asset of {repo} {tag} matches; assets: {}",
                    names.join(", ")
                ),
            });
        }
        many => {
            let m: Vec<&str> = many.iter().filter_map(|a| a["name"].as_str()).collect();
            return Err(Error::Catalog {
                entry: emulator.into(),
                why: format!("{} assets of {repo} {tag} match: {}", m.len(), m.join(", ")),
            });
        }
    };
    let digest = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_string);
    Ok(Resolved {
        emulator: emulator.into(),
        channel: "github".into(),
        url: asset["browser_download_url"].as_str().map(str::to_string),
        file_name: asset["name"].as_str().map(str::to_string),
        size: asset["size"].as_u64(),
        version: version_from_tag(&tag),
        release: release_id(&tag, &date, asset, digest.as_deref()),
        digest,
    })
}

/// `<tag>@<asset updated_at>#<asset id>:<digest prefix>`. The tag and its publish date are
/// not enough: a rolling asset (Vita3K's `windows-latest.zip`) is rebuilt in place, the same
/// day, and only the asset says so.
fn release_id(tag: &str, date: &str, asset: &serde_json::Value, digest: Option<&str>) -> String {
    let mut id = format!("{tag}@{}", asset["updated_at"].as_str().unwrap_or(date));
    if let Some(n) = asset["id"].as_u64() {
        id.push_str(&format!("#{n}"));
    }
    if let Some(d) = digest {
        id.push(':');
        id.extend(d.chars().take(16));
    }
    id
}

/// Checksums and signatures published beside an asset are never the asset.
fn is_sidecar(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        ".sha256",
        ".sha256sum",
        ".sha512",
        ".sig",
        ".asc",
        ".zsync",
        ".sha1",
        ".md5",
    ]
    .iter()
    .any(|s| n.ends_with(s))
}

/// `v2.8.2` → `2.8.2`; a rolling tag → none.
pub fn version_from_tag(tag: &str) -> Option<String> {
    let rolling = matches!(tag, "latest" | "continuous" | "nightly")
        || tag.starts_with("build-")
        || (tag.len() >= 7
            && tag.chars().all(|c| c.is_ascii_hexdigit())
            && !tag.chars().all(|c| c.is_ascii_digit()));
    if rolling {
        None
    } else {
        Some(tag.trim_start_matches('v').to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::http::fake::FakeHttp;
    use serde_json::json;

    fn release(tag: &str, assets: &[(&str, Option<&str>)]) -> serde_json::Value {
        json!({
            "tag_name": tag,
            "published_at": "2026-09-20T10:00:00Z",
            "assets": assets.iter().map(|(n, d)| json!({
                "name": n, "size": 10, "browser_download_url": format!("https://x/{n}"),
                "digest": d.map(|d| format!("sha256:{d}")),
            })).collect::<Vec<_>>()
        })
    }

    #[test]
    fn picks_exactly_one_asset() {
        let mut http = FakeHttp::default();
        http.json.insert(
            "https://api.github.com/repos/PCSX2/pcsx2/releases/latest".into(),
            release(
                "v2.8.2",
                &[
                    ("pcsx2-v2.8.2-windows-x64-Qt.7z", Some("ab")),
                    ("pcsx2-v2.8.2-windows-x64-installer.exe", None),
                ],
            ),
        );
        let f = AssetFilter::Filter {
            all: vec!["windows-x64-qt".into(), ".7z".into()],
            none: vec![],
        };
        let r = resolve(&http, "pcsx2", "PCSX2/pcsx2", &f).unwrap();
        assert_eq!(r.version.as_deref(), Some("2.8.2"));
        assert_eq!(r.release, "v2.8.2@2026-09-20:ab");
        assert_eq!(r.size, Some(10));
        assert_eq!(r.digest.as_deref(), Some("ab"));
        assert_eq!(
            r.file_name.as_deref(),
            Some("pcsx2-v2.8.2-windows-x64-Qt.7z")
        );

        let f = AssetFilter::Filter {
            all: vec!["windows".into()],
            none: vec![],
        };
        assert!(matches!(
            resolve(&http, "pcsx2", "PCSX2/pcsx2", &f),
            Err(Error::Catalog { .. })
        ));
        let f = AssetFilter::Name {
            name: "nope.zip".into(),
        };
        assert!(matches!(
            resolve(&http, "pcsx2", "PCSX2/pcsx2", &f),
            Err(Error::Catalog { .. })
        ));
    }

    /// Vita3K's `windows-latest.zip`: the same tag, published the same day, rebuilt in place.
    #[test]
    fn a_rolling_asset_rebuilt_the_same_day_is_a_new_release() {
        let rel = |updated: &str, id: u64, digest: &str| {
            json!({
                "tag_name": "continuous",
                "published_at": "2026-09-20T01:00:00Z",
                "assets": [{
                    "name": "windows-latest.zip", "size": 10, "id": id, "updated_at": updated,
                    "browser_download_url": "https://x/windows-latest.zip",
                    "digest": format!("sha256:{digest}"),
                }]
            })
        };
        let f = AssetFilter::Name {
            name: "windows-latest.zip".into(),
        };
        let url = "https://api.github.com/repos/Vita3K/Vita3K/releases/latest";
        let mut http = FakeHttp::default();
        http.json
            .insert(url.into(), rel("2026-09-20T02:00:00Z", 7, &"a".repeat(64)));
        let morning = resolve(&http, "vita3k", "Vita3K/Vita3K", &f).unwrap();
        assert_eq!(
            morning.release,
            "continuous@2026-09-20T02:00:00Z#7:aaaaaaaaaaaaaaaa"
        );
        http.json
            .insert(url.into(), rel("2026-09-20T18:00:00Z", 8, &"b".repeat(64)));
        let evening = resolve(&http, "vita3k", "Vita3K/Vita3K", &f).unwrap();
        assert_ne!(morning.release, evening.release);
    }

    #[test]
    fn sidecars_never_match() {
        assert!(is_sidecar("rpcs3-v0.0.42_win64_msvc.7z.sha256"));
        assert!(is_sidecar("PPSSPP.AppImage.zsync"));
        assert!(!is_sidecar("rpcs3-v0.0.42_win64_msvc.7z"));
    }

    #[test]
    fn rolling_tags_have_no_version() {
        assert_eq!(version_from_tag("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(version_from_tag("2126.1.2").as_deref(), Some("2126.1.2"));
        assert_eq!(version_from_tag("latest"), None);
        assert_eq!(version_from_tag("continuous"), None);
        assert_eq!(
            version_from_tag("build-205a4481f3469953889df85c177e45ca8f199756"),
            None
        );
        assert_eq!(version_from_tag("02d2cb5"), None);
    }
}
