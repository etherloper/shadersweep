//! Reads Steam's `appcache/appinfo.vdf`: a binary cache describing every app the account
//! has seen, installed or not. We take each app's name, install folder and icon so caches
//! from uninstalled games can still be shown with their real name and icon.
//!
//! Layout (versions 28 and 29, magic `0x07564428` / `0x07564429`):
//! header `magic u32, universe u32, [v29: string_table_offset i64]`, then per app
//! `appid u32 (0 ends), size u32, 60 bytes of metadata, binary KeyValues`. In v29 keys are
//! u32 indexes into a string table stored at `string_table_offset`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAGIC_V28: u32 = 0x0756_4428;
const MAGIC_V29: u32 = 0x0756_4429;
const ENTRY_METADATA_LEN: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct SteamApp {
    pub app_id: u32,
    pub name: String,
    pub install_dir: Option<String>,
    /// `common/icon`: names the small `.jpg` in the library cache.
    pub icon_hash: Option<String>,
    /// `common/clienticon`: names the `.ico` Steam writes for desktop shortcuts.
    pub client_icon_hash: Option<String>,
}

#[derive(Debug)]
enum Value {
    Text(String),
    Object(Vec<(String, Value)>),
    Other,
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(entries) => entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let bytes = self.data.get(self.pos..self.pos.checked_add(len)?)?;
        self.pos += len;
        Some(bytes)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
    }

    fn cstring(&mut self) -> Option<String> {
        let rest = self.data.get(self.pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        self.pos += end + 1;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

/// Keys are inline C strings (v28) or indexes into the string table (v29).
enum Keys<'a> {
    Inline,
    Table(&'a [String]),
}

impl Keys<'_> {
    fn read(&self, reader: &mut Reader) -> Option<String> {
        match self {
            Keys::Inline => reader.cstring(),
            Keys::Table(table) => table.get(reader.u32()? as usize).cloned(),
        }
    }
}

fn read_object(reader: &mut Reader, keys: &Keys, end: usize, depth: usize) -> Option<Value> {
    if depth > 32 {
        return None;
    }
    let mut entries = Vec::new();
    while reader.pos < end {
        let kind = reader.u8()?;
        if kind == 0x08 {
            return Some(Value::Object(entries));
        }
        let key = keys.read(reader)?;
        let value = match kind {
            0x00 => read_object(reader, keys, end, depth + 1)?,
            0x01 => Value::Text(reader.cstring()?),
            0x02 | 0x03 | 0x04 | 0x06 => {
                reader.take(4)?;
                Value::Other
            }
            0x07 | 0x0a => {
                reader.u64()?;
                Value::Other
            }
            _ => return None, // unknown type: give up on this app rather than misread
        };
        entries.push((key, value));
    }
    Some(Value::Object(entries))
}

fn parse(data: &[u8]) -> Option<Vec<SteamApp>> {
    let mut reader = Reader { data, pos: 0 };
    let magic = reader.u32()?;
    let _universe = reader.u32()?;
    let table: Vec<String>;
    let keys = match magic {
        MAGIC_V28 => Keys::Inline,
        MAGIC_V29 => {
            let offset = usize::try_from(reader.u64()?).ok()?;
            let mut table_reader = Reader { data, pos: offset };
            let count = table_reader.u32()? as usize;
            table = (0..count).map(|_| table_reader.cstring()).collect::<Option<_>>()?;
            Keys::Table(&table)
        }
        _ => return None,
    };

    let mut apps = Vec::new();
    loop {
        let app_id = reader.u32()?;
        if app_id == 0 {
            break;
        }
        let size = reader.u32()? as usize;
        let entry_end = reader.pos.checked_add(size)?;
        if entry_end > data.len() {
            break;
        }
        reader.pos += ENTRY_METADATA_LEN;
        if let Some(app) = read_object(&mut reader, &keys, entry_end, 0).and_then(|root| to_app(app_id, &root)) {
            apps.push(app);
        }
        reader.pos = entry_end;
    }
    Some(apps)
}

fn to_app(app_id: u32, root: &Value) -> Option<SteamApp> {
    let info = root.get("appinfo").unwrap_or(root);
    let common = info.get("common")?;
    let name = common.get("name")?.text()?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    Some(SteamApp {
        app_id,
        name,
        install_dir: info.get("config").and_then(|c| c.get("installdir")).and_then(Value::text).map(str::to_owned),
        icon_hash: common.get("icon").and_then(Value::text).map(str::to_owned),
        client_icon_hash: common.get("clienticon").and_then(Value::text).map(str::to_owned),
    })
}

/// The best icon Steam has on disk for an app, trying the documented names first and then
/// any hash-named `.jpg` in the app's library cache folder (the small icon).
fn find_icon(steam_root: &Path, app: &SteamApp) -> Option<PathBuf> {
    let library_cache = steam_root.join("appcache").join("librarycache");
    let app_folder = library_cache.join(app.app_id.to_string());
    let named = [
        app.icon_hash.as_ref().map(|hash| app_folder.join(format!("{hash}.jpg"))),
        Some(library_cache.join(format!("{}_icon.jpg", app.app_id))),
        app.client_icon_hash.as_ref().map(|hash| steam_root.join("steam").join("games").join(format!("{hash}.ico"))),
    ];
    if let Some(path) = named.into_iter().flatten().find(|path| path.is_file()) {
        return Some(path);
    }
    fs::read_dir(&app_folder).ok()?.flatten().map(|entry| entry.path()).find(|path| {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let is_jpg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg"));
        is_jpg && stem.len() == 40 && stem.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Steam apps keyed by lowercased install folder name (`steamapps\common\<folder>`).
pub type SteamCatalog = HashMap<String, SteamCatalogEntry>;

#[derive(Clone, Debug)]
pub struct SteamCatalogEntry {
    pub name: String,
    /// Cached artwork icon, if Steam has one on disk.
    pub icon: Option<PathBuf>,
}

pub fn load_catalog(steam_root: &Path) -> io::Result<SteamCatalog> {
    let data = fs::read(steam_root.join("appcache").join("appinfo.vdf"))?;
    let apps = parse(&data).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unrecognised appinfo.vdf"))?;
    let mut catalog = SteamCatalog::new();
    for app in apps {
        let Some(dir) = app.install_dir.as_deref().map(str::to_lowercase) else { continue };
        if catalog.contains_key(&dir) {
            continue;
        }
        let icon = find_icon(steam_root, &app);
        catalog.insert(dir, SteamCatalogEntry { name: app.name, icon });
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal v29 file with one app, mirroring Steam's layout.
    fn sample_v29() -> Vec<u8> {
        let strings = ["appinfo", "common", "name", "icon", "config", "installdir", "type"];
        let key = |s: &str| (strings.iter().position(|k| *k == s).unwrap() as u32).to_le_bytes();
        let mut kv = Vec::new();
        kv.push(0x00);
        kv.extend(key("appinfo"));
        kv.push(0x00);
        kv.extend(key("common"));
        kv.push(0x01);
        kv.extend(key("name"));
        kv.extend(b"ARC Raiders\0");
        kv.push(0x01);
        kv.extend(key("icon"));
        kv.extend(b"c284e73b\0");
        kv.push(0x02);
        kv.extend(key("type"));
        kv.extend(7u32.to_le_bytes());
        kv.push(0x08);
        kv.push(0x00);
        kv.extend(key("config"));
        kv.push(0x01);
        kv.extend(key("installdir"));
        kv.extend(b"Arc Raiders\0");
        kv.push(0x08);
        kv.push(0x08);
        kv.push(0x08);

        let mut entry = vec![0u8; ENTRY_METADATA_LEN];
        entry.extend(&kv);

        let mut file = Vec::new();
        file.extend(MAGIC_V29.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        let table_offset_pos = file.len();
        file.extend(0u64.to_le_bytes());
        file.extend(1_808_500u32.to_le_bytes());
        file.extend((entry.len() as u32).to_le_bytes());
        file.extend(&entry);
        file.extend(0u32.to_le_bytes());
        let table_offset = file.len() as u64;
        file[table_offset_pos..table_offset_pos + 8].copy_from_slice(&table_offset.to_le_bytes());
        file.extend((strings.len() as u32).to_le_bytes());
        for s in strings {
            file.extend(s.as_bytes());
            file.push(0);
        }
        file
    }

    #[test]
    fn parses_v29_appinfo() {
        let apps = parse(&sample_v29()).unwrap();
        assert_eq!(
            apps,
            [SteamApp {
                app_id: 1_808_500,
                name: "ARC Raiders".into(),
                install_dir: Some("Arc Raiders".into()),
                icon_hash: Some("c284e73b".into()),
                client_icon_hash: None,
            }]
        );
    }

    #[test]
    fn rejects_unknown_files_without_panicking() {
        assert!(parse(b"not an appinfo file").is_none());
        let mut truncated = sample_v29();
        truncated.truncate(40);
        assert!(parse(&truncated).is_none_or(|apps| apps.is_empty()));
    }
}
