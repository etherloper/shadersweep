//! Update checks against GitHub Releases, and in-place installation.
//!
//! A release is expected to carry two assets, produced by `.github/workflows/release.yml`:
//! `shadersweep.exe` and `shadersweep.exe.sha256`. The download is verified against that
//! checksum (and the asset size) before it replaces the running executable. Windows allows
//! renaming a running exe, so the swap is: current → `.old`, download → current; the `.old`
//! file is removed on the next start.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::http;

/// Where releases are published.
pub const REPOSITORY: &str = "etherloper/shadersweep";
/// Overrides the "latest release" API URL, e.g. to test against a local server.
const URL_OVERRIDE_VAR: &str = "SHADERSWEEP_UPDATE_URL";
const EXE_ASSET: &str = "shadersweep.exe";
const CHECKSUM_ASSET: &str = "shadersweep.exe.sha256";

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn user_agent() -> String {
    format!("Shadersweep/{}", current_version())
}

fn latest_release_url() -> String {
    std::env::var(URL_OVERRIDE_VAR)
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"))
}

pub fn releases_page() -> String {
    format!("https://github.com/{REPOSITORY}/releases")
}

#[derive(Clone, Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    exe_url: String,
    exe_size: u64,
    checksum_url: String,
}

impl Release {
    pub fn exe_size(&self) -> u64 {
        self.exe_size
    }
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next().unwrap_or(Some(0))?, parts.next().unwrap_or(Some(0))?);
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Release notes are Markdown written for GitHub, but the update card shows a few lines of
/// plain text. Markup is stripped, and lines that only point back to GitHub (the "Full
/// Changelog" link, section headings, contributor credits) are dropped: the card has its own
/// Release notes button.
fn plain_notes(markdown: &str) -> String {
    markdown.lines().filter_map(plain_line).collect::<Vec<_>>().join("\n")
}

fn plain_line(line: &str) -> Option<String> {
    let line = line.trim();
    let (bullet, rest) = match line.strip_prefix("* ").or_else(|| line.strip_prefix("- ")) {
        Some(rest) => ("• ", rest),
        None => ("", line.trim_start_matches('#').trim_start()),
    };
    let mut text = strip_links(rest).replace("**", "").replace("__", "").replace('`', "");
    // GitHub's generated entries end with "by @author in <pull request URL>".
    if let Some(credit) = text.find(" by @") {
        text.truncate(credit);
    }
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let github_only = lower.starts_with("full changelog")
        || lower == "what's changed"
        || lower == "new contributors"
        || lower.contains("made their first contribution");
    (!text.is_empty() && !github_only).then(|| format!("{bullet}{text}"))
}

/// `[label](url)` becomes `label`.
fn strip_links(text: &str) -> String {
    let mut plain = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(middle) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(close) = rest[middle..].find(')').map(|i| middle + i) else { break };
        plain.push_str(&rest[..open]);
        plain.push_str(&rest[open + 1..middle]);
        rest = &rest[close + 1..];
    }
    plain.push_str(rest);
    plain
}

fn parse_release(json: &str) -> Result<Release, String> {
    let release: GithubRelease =
        serde_json::from_str(json).map_err(|err| format!("unexpected response from GitHub ({err})"))?;
    let version = release.tag_name.trim_start_matches(['v', 'V']).to_owned();
    if parse_version(&version).is_none() {
        return Err(format!("release tag \"{}\" is not a version number", release.tag_name));
    }
    let asset = |name: &str| release.assets.iter().find(|a| a.name.eq_ignore_ascii_case(name));
    let exe = asset(EXE_ASSET).ok_or_else(|| format!("release {version} has no {EXE_ASSET} download"))?;
    let checksum = asset(CHECKSUM_ASSET).ok_or_else(|| format!("release {version} has no checksum file"))?;
    Ok(Release {
        version,
        notes: plain_notes(release.body.as_deref().unwrap_or_default()),
        page_url: release.html_url,
        exe_url: exe.browser_download_url.clone(),
        exe_size: exe.size,
        checksum_url: checksum.browser_download_url.clone(),
    })
}

/// Reads the hex digest from a `sha256sum`-style line: `<hex>  <file name>`.
fn parse_checksum(text: &str) -> Option<String> {
    let digest = text.split_whitespace().next()?.to_ascii_lowercase();
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

fn get_ok(url: &str, accept: &str) -> Result<http::Response, String> {
    let response = http::get(url, &user_agent(), accept).map_err(|err| err.to_string())?;
    match response.status {
        200 => Ok(response),
        404 => Err("no published release was found".into()),
        403 | 429 => Err("GitHub is rate-limiting update checks; try again later".into()),
        status => Err(format!("the update server answered with HTTP {status}")),
    }
}

/// Returns the latest release if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let mut body = String::new();
    get_ok(&latest_release_url(), "application/vnd.github+json")?
        .read_to_string(&mut body)
        .map_err(|err| err.to_string())?;
    let release = parse_release(&body)?;
    Ok(is_newer(&release.version, current_version()).then_some(release))
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Downloads, verifies and swaps in the new executable. `progress` receives the number of
/// bytes downloaded so far. Returns the path to launch to run the new version.
pub fn install(release: &Release, progress: Arc<AtomicU64>) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|err| format!("can't locate Shadersweep's exe: {err}"))?;
    install_to(&current, release, &progress)
}

fn install_to(target: &Path, release: &Release, progress: &AtomicU64) -> Result<PathBuf, String> {
    let mut checksum_text = String::new();
    get_ok(&release.checksum_url, "text/plain, application/octet-stream")?
        .read_to_string(&mut checksum_text)
        .map_err(|err| err.to_string())?;
    let expected = parse_checksum(&checksum_text).ok_or("the release's checksum file is malformed")?;

    let staged = sibling(target, ".new");
    let downloaded = download_verified(release, &expected, &staged, progress);
    if let Err(err) = downloaded {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }
    swap_in(target, &staged)?;
    Ok(target.to_path_buf())
}

fn download_verified(release: &Release, expected: &str, staged: &Path, progress: &AtomicU64) -> Result<(), String> {
    let mut response = get_ok(&release.exe_url, "application/octet-stream")?;
    let mut file = File::create(staged).map_err(|err| match err.kind() {
        io::ErrorKind::PermissionDenied => format!(
            "Shadersweep can't write to its own folder ({}). Download the update manually instead.",
            staged.parent().map(|p| p.display().to_string()).unwrap_or_default()
        ),
        _ => format!("couldn't save the download: {err}"),
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = response.read(&mut buf).map_err(|err| err.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|err| format!("couldn't save the download: {err}"))?;
        total += n as u64;
        progress.store(total, Ordering::Relaxed);
    }
    file.sync_all().map_err(|err| format!("couldn't save the download: {err}"))?;
    if total != release.exe_size {
        return Err(format!("download was incomplete ({total} of {} bytes)", release.exe_size));
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err("the download didn't match its checksum, so it was discarded".into());
    }
    Ok(())
}

fn swap_in(target: &Path, staged: &Path) -> Result<(), String> {
    let old = sibling(target, ".old");
    let _ = fs::remove_file(&old);
    fs::rename(target, &old).map_err(|err| format!("couldn't move the current version aside: {err}"))?;
    if let Err(err) = fs::rename(staged, target) {
        // Put the working version back so the app still starts next time.
        let _ = fs::rename(&old, target);
        return Err(format!("couldn't put the new version in place: {err}"));
    }
    Ok(())
}

/// Removes leftovers from a previous update (`.old` from the swap, `.new` from a failed
/// download). Right after "Restart now" the previous process may still be exiting and
/// holding `.old`, so this retries briefly on a background thread; anything still locked
/// after that is tried again on the next start.
pub fn clean_up_previous_install() {
    const ATTEMPTS: u32 = 20;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

    let Ok(current) = std::env::current_exe() else { return };
    let leftovers: Vec<PathBuf> = [".old", ".new"]
        .into_iter()
        .map(|suffix| sibling(&current, suffix))
        .filter(|path| path.exists())
        .collect();
    if leftovers.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for path in leftovers {
            for attempt in 1..=ATTEMPTS {
                match fs::remove_file(&path) {
                    Ok(()) => break,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => break,
                    Err(err) if attempt == ATTEMPTS => {
                        eprintln!("could not remove {}: {err}", path.display());
                    }
                    Err(_) => std::thread::sleep(RETRY_DELAY),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.4"), Some((0, 4, 0)));
        assert_eq!(parse_version("1.0.0-beta.1"), Some((1, 0, 0)));
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(is_newer("v0.4.0", "0.3.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(!is_newer("garbage", "0.3.0"));
    }

    #[test]
    fn parses_github_release() {
        let json = r#"{
            "tag_name": "v0.4.0",
            "html_url": "https://github.com/o/r/releases/tag/v0.4.0",
            "body": "  Faster scans.\n",
            "assets": [
                {"name": "shadersweep.exe", "browser_download_url": "https://x/exe", "size": 123},
                {"name": "shadersweep.exe.sha256", "browser_download_url": "https://x/sha", "size": 80}
            ]
        }"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, "0.4.0");
        assert_eq!(release.notes, "Faster scans.");
        assert_eq!(release.exe_size, 123);
        assert_eq!(release.checksum_url, "https://x/sha");
    }

    #[test]
    fn rejects_releases_without_assets() {
        let json = r#"{"tag_name":"v0.4.0","html_url":"u","assets":[]}"#;
        assert!(parse_release(json).unwrap_err().contains("no shadersweep.exe"));
    }

    #[test]
    fn parses_checksum_lines() {
        let hex = "a".repeat(64);
        assert_eq!(parse_checksum(&format!("{hex}  shadersweep.exe\n")), Some(hex.clone()));
        assert_eq!(parse_checksum(&hex.to_uppercase()), Some(hex));
        assert_eq!(parse_checksum("abc shadersweep.exe"), None);
    }

    #[test]
    fn swap_replaces_target_and_keeps_old_copy() {
        let dir = std::env::temp_dir().join(format!("scv-swap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("app.exe");
        let staged = sibling(&target, ".new");
        fs::write(&target, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();

        swap_in(&target, &staged).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read(sibling(&target, ".old")).unwrap(), b"old");
        assert!(!staged.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn release_notes_become_plain_text() {
        assert_eq!(
            plain_notes("**Full Changelog**: https://github.com/etherloper/shadersweep/compare/v0.1.3...v0.1.4"),
            ""
        );
        let generated = "## What's Changed\n\
            * Treemap overview by @etherloper in https://github.com/etherloper/shadersweep/pull/4\n\
            \n\
            ## New Contributors\n\
            * @someone made their first contribution in https://github.com/etherloper/shadersweep/pull/5\n\
            \n\
            **Full Changelog**: https://github.com/etherloper/shadersweep/compare/v0.1.3...v0.1.4";
        assert_eq!(plain_notes(generated), "• Treemap overview");
        let written = "### Highlights\n- **Faster** scans with `rayon`\n- See [the docs](https://example.com) for more";
        assert_eq!(plain_notes(written), "Highlights\n• Faster scans with rayon\n• See the docs for more");
    }
}
