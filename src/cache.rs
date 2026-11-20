//! Reading and grouping NVIDIA's DirectX shader cache (`%LOCALAPPDATA%\NVIDIA\DXCache`).
//!
//! The `.nvph` format is undocumented. What we rely on, observed on driver 5xx/6xx:
//! - every file starts with the magic bytes `nvph`;
//! - within the first few dozen bytes there is a NUL-terminated, lowercase name of the
//!   process that produced it (an `.exe` name, or a package family name for Store apps);
//! - file stems are 16 hex digits, and the last 8 identify the application, so a file we
//!   cannot read (locked by a running game) can still be attributed through a sibling.
//!
//! Anything that does not fit lands in an "unidentified" group instead of failing the scan.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAGIC: &[u8; 4] = b"nvph";
const HEADER_PROBE_LEN: usize = 512;
const MIN_NAME_LEN: usize = 3;
pub const CACHE_EXTENSION: &str = "nvph";
pub const UNIDENTIFIED_KEY: &str = "(unidentified)";

/// Windows error codes for a file opened by another process without sharing.
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// `SHADER_CACHE_DIR` overrides the location, e.g. to try the app on a copy of the cache.
pub fn default_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SHADER_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("NVIDIA").join("DXCache"))
}

#[derive(Clone, Debug)]
pub struct CacheFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The file could not be opened because a running process holds it.
    pub locked: bool,
}

#[derive(Clone, Debug)]
pub struct AppCache {
    /// Process name as stored by the driver, or [`UNIDENTIFIED_KEY`].
    pub key: String,
    pub files: Vec<CacheFile>,
    pub total_size: u64,
    pub last_modified: Option<SystemTime>,
}

impl AppCache {
    pub fn is_identified(&self) -> bool {
        self.key != UNIDENTIFIED_KEY
    }

    pub fn locked_count(&self) -> usize {
        self.files.iter().filter(|f| f.locked).count()
    }
}

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub dir: PathBuf,
    /// Sorted by total size, largest first.
    pub apps: Vec<AppCache>,
    pub total_size: u64,
    pub file_count: usize,
}

/// Extracts the producing process name from the start of an `.nvph` file.
pub fn parse_app_name(header: &[u8]) -> Option<String> {
    let body = header.strip_prefix(MAGIC)?;
    let mut run_start = None;
    for (i, &byte) in body.iter().enumerate() {
        let is_name_byte = byte >= 0x20 && byte != 0x7f;
        if is_name_byte {
            run_start.get_or_insert(i);
            continue;
        }
        if let Some(start) = run_start.take()
            && byte == 0
            && let Some(name) = as_process_name(&body[start..i])
        {
            return Some(name);
        }
    }
    None
}

/// Header fields before the name can contain bytes that look like text, so a candidate
/// must be valid UTF-8 and contain a dot: every real name is `x.exe` or `pkg.name_hash`.
fn as_process_name(bytes: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(bytes).ok()?.trim();
    (name.len() >= MIN_NAME_LEN && name.contains('.')).then(|| name.to_lowercase())
}

/// The application part of a cache file name (`0002a91d4dfbeada` → `4dfbeada`).
fn app_suffix(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let is_hex_stem = stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit());
    is_hex_stem.then(|| stem[8..].to_ascii_lowercase())
}

#[derive(Debug)]
struct ScannedEntry {
    file: CacheFile,
    name: Option<String>,
    suffix: Option<String>,
}

enum HeaderRead {
    Read(Vec<u8>),
    Locked,
    Unreadable,
}

fn read_header(path: &Path) -> HeaderRead {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if is_lock_error(&err) => return HeaderRead::Locked,
        Err(_) => return HeaderRead::Unreadable,
    };
    let mut buf = Vec::with_capacity(HEADER_PROBE_LEN);
    match file.by_ref().take(HEADER_PROBE_LEN as u64).read_to_end(&mut buf) {
        Ok(_) => HeaderRead::Read(buf),
        Err(err) if is_lock_error(&err) => HeaderRead::Locked,
        Err(_) => HeaderRead::Unreadable,
    }
}

fn is_lock_error(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION))
}

fn is_cache_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(CACHE_EXTENSION))
}

pub fn scan(dir: &Path) -> io::Result<ScanResult> {
    let mut entries = Vec::new();
    for dir_entry in fs::read_dir(dir)? {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if !is_cache_file(&path) {
            continue;
        }
        let Ok(metadata) = dir_entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        let (name, locked) = match read_header(&path) {
            HeaderRead::Read(header) => (parse_app_name(&header), false),
            HeaderRead::Locked => (None, true),
            HeaderRead::Unreadable => (None, false),
        };
        entries.push(ScannedEntry {
            suffix: app_suffix(&path),
            name,
            file: CacheFile {
                path,
                size: metadata.len(),
                modified: metadata.modified().ok(),
                locked,
            },
        });
    }
    Ok(group(dir.to_path_buf(), entries))
}

fn group(dir: PathBuf, entries: Vec<ScannedEntry>) -> ScanResult {
    let name_by_suffix: HashMap<String, String> = entries
        .iter()
        .filter_map(|e| Some((e.suffix.clone()?, e.name.clone()?)))
        .collect();

    let mut apps: HashMap<String, AppCache> = HashMap::new();
    let mut total_size = 0;
    let file_count = entries.len();
    for entry in entries {
        let key = entry
            .name
            .or_else(|| entry.suffix.and_then(|s| name_by_suffix.get(&s).cloned()))
            .unwrap_or_else(|| UNIDENTIFIED_KEY.to_owned());
        let app = apps.entry(key.clone()).or_insert_with(|| AppCache {
            key,
            files: Vec::new(),
            total_size: 0,
            last_modified: None,
        });
        total_size += entry.file.size;
        app.total_size += entry.file.size;
        app.last_modified = app.last_modified.max(entry.file.modified);
        app.files.push(entry.file);
    }

    let mut apps: Vec<AppCache> = apps.into_values().collect();
    for app in &mut apps {
        app.files.sort_by_key(|f| std::cmp::Reverse(f.size));
    }
    apps.sort_by(|a, b| b.total_size.cmp(&a.total_size).then_with(|| a.key.cmp(&b.key)));
    ScanResult { dir, apps, total_size, file_count }
}

#[derive(Debug, Default)]
pub struct DeleteReport {
    pub deleted: usize,
    pub freed: u64,
    /// Files still held open by a running process.
    pub in_use: usize,
    pub failed: Vec<(PathBuf, String)>,
}

/// Deletes the given cache files. Paths outside `cache_dir`, or that are not `.nvph`
/// files, are refused so a bad selection can never touch anything else on disk.
pub fn delete_files(cache_dir: &Path, files: &[CacheFile]) -> DeleteReport {
    let mut report = DeleteReport::default();
    for file in files {
        let inside_cache = file.path.parent() == Some(cache_dir) && is_cache_file(&file.path);
        if !inside_cache {
            report.failed.push((file.path.clone(), "outside the shader cache folder".into()));
            continue;
        }
        match fs::remove_file(&file.path) {
            Ok(()) => {
                report.deleted += 1;
                report.freed += file.size;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) if is_lock_error(&err) || err.kind() == io::ErrorKind::PermissionDenied => {
                report.in_use += 1;
            }
            Err(err) => report.failed.push((file.path.clone(), err.to_string())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(bytes: &[u8]) -> Vec<u8> {
        let mut h = bytes.to_vec();
        h.resize(HEADER_PROBE_LEN, 0);
        h
    }

    #[test]
    fn parses_common_layout() {
        // From 791fa91d3a3576d3.nvph: u32 at 8, zero u32, flag byte, then the name.
        let h = header(b"nvph\0\0\0\0\x05\x4d\xaf\x02\0\0\0\0\x01totclient-win64-shipping.exe\0");
        assert_eq!(parse_app_name(&h).as_deref(), Some("totclient-win64-shipping.exe"));
    }

    #[test]
    fn parses_shifted_layout() {
        // From 7e55a91d12df60d8.nvph, where the name starts at 0x0d.
        let h = header(b"nvph\0\0\0\0\xeb\x5a\x04\0\0battle.net.exe\0");
        assert_eq!(parse_app_name(&h).as_deref(), Some("battle.net.exe"));
    }

    #[test]
    fn parses_store_app_package_name() {
        let h = header(b"nvph\0\0\0\0\xd0\x05\0\0\0\0\0\0\x01microsoftwindows.client.cbs_cw5n1h2txyewy\0");
        assert_eq!(
            parse_app_name(&h).as_deref(),
            Some("microsoftwindows.client.cbs_cw5n1h2txyewy")
        );
    }

    #[test]
    fn spaces_and_case_are_normalised() {
        let h = header(b"nvph\0\0\0\0\x00\x1e\0\0\0\0\0\0\x01Ignite The Depths Demo.exe\0");
        assert_eq!(parse_app_name(&h).as_deref(), Some("ignite the depths demo.exe"));
    }

    #[test]
    fn rejects_wrong_magic_and_unterminated_names() {
        assert_eq!(parse_app_name(&header(b"abcd\0\0\0\0\x01game.exe\0")), None);
        assert_eq!(parse_app_name(b"nvph\0\0\0\0\x01game.exe"), None);
        assert_eq!(parse_app_name(b"nv"), None);
    }

    #[test]
    fn ignores_short_printable_noise() {
        // 'Z' (0x5a) in the size field must not be taken as a name.
        let h = header(b"nvph\0\0\0\0\xeb\x5a\0\0\0\0\0\0\x01re8.exe\0");
        assert_eq!(parse_app_name(&h).as_deref(), Some("re8.exe"));
    }

    #[test]
    fn skips_binary_size_fields_that_look_like_text() {
        // Large files have size fields such as 64 e9 7e f3 or 41 42 43 that precede a NUL.
        let h = header(b"nvph\0\0\0\0d\xe9~\xf3\0\0\0\0\x01goweday-steam.exe\0");
        assert_eq!(parse_app_name(&h).as_deref(), Some("goweday-steam.exe"));
        let h = header(b"nvph\0\0\0\0ABCD\0\0\0\0\x01pbz.exe\0");
        assert_eq!(parse_app_name(&h).as_deref(), Some("pbz.exe"));
    }

    #[test]
    fn suffix_requires_hex_stem() {
        assert_eq!(app_suffix(Path::new("0002a91d4dfbeada.nvph")).as_deref(), Some("4dfbeada"));
        assert_eq!(app_suffix(Path::new("notes.nvph")), None);
    }

    fn entry(name: &str, size: u64, parsed: Option<&str>, locked: bool) -> ScannedEntry {
        let path = PathBuf::from(format!("C:/cache/{name}.nvph"));
        ScannedEntry {
            suffix: app_suffix(&path),
            name: parsed.map(str::to_owned),
            file: CacheFile { path, size, modified: None, locked },
        }
    }

    #[test]
    fn locked_files_are_attributed_through_their_suffix() {
        let result = group(
            PathBuf::from("C:/cache"),
            vec![
                entry("0000a91d0467342d", 10, Some("game.exe"), false),
                entry("e1efa91d0467342d", 500, None, true),
                entry("0000a91d128cd128", 5, None, true),
                entry("0000a91d99999999", 7, Some("other.exe"), false),
            ],
        );
        assert_eq!(result.total_size, 522);
        assert_eq!(result.file_count, 4);
        let keys: Vec<&str> = result.apps.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["game.exe", "other.exe", UNIDENTIFIED_KEY]);
        let game = &result.apps[0];
        assert_eq!(game.total_size, 510);
        assert_eq!(game.locked_count(), 1);
        assert_eq!(game.files[0].size, 500, "files are sorted largest first");
    }

    #[test]
    fn delete_refuses_paths_outside_cache_dir() {
        let dir = std::env::temp_dir().join(format!("scv-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let inside = dir.join("0000a91d00000001.nvph");
        fs::write(&inside, b"nvph").unwrap();
        let outside = std::env::temp_dir().join(format!("scv-outside-{}.nvph", std::process::id()));
        fs::write(&outside, b"nvph").unwrap();
        let wrong_ext = dir.join("keep.txt");
        fs::write(&wrong_ext, b"x").unwrap();

        let as_file = |path: &Path| CacheFile { path: path.to_path_buf(), size: 4, modified: None, locked: false };
        let report = delete_files(&dir, &[as_file(&inside), as_file(&outside), as_file(&wrong_ext)]);

        assert_eq!(report.deleted, 1);
        assert_eq!(report.freed, 4);
        assert_eq!(report.failed.len(), 2);
        assert!(!inside.exists());
        assert!(outside.exists() && wrong_ext.exists());
        fs::remove_file(&outside).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
