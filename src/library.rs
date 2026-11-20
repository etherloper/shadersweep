//! Maps executable names to friendly game titles using locally installed launchers
//! (Steam and Epic). Purely best-effort: anything missing just shows its exe name.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// How deep to look for executables under a game's install folder. Unreal titles keep
/// theirs at `<Project>/Binaries/Win64/<Name>-Win64-Shipping.exe`.
const EXE_SEARCH_DEPTH: usize = 4;
const DEFAULT_STEAM_DIR: &str = r"C:\Program Files (x86)\Steam";
const EPIC_MANIFEST_DIR: &str = r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests";

#[derive(Clone, Debug)]
pub struct GameInfo {
    pub title: String,
    pub exe_path: PathBuf,
}

/// Lowercased exe file name → installed game.
pub type GameLibrary = HashMap<String, GameInfo>;

pub fn discover_games() -> GameLibrary {
    let mut games = GameLibrary::new();
    for (title, install_dir) in steam_games().into_iter().chain(epic_games()) {
        for (exe, exe_path) in find_exes(&install_dir, EXE_SEARCH_DEPTH) {
            games.entry(exe).or_insert_with(|| GameInfo { title: title.clone(), exe_path });
        }
    }
    games
}

/// `(lowercased file name, full path)` of every `.exe` under `dir`.
fn find_exes(dir: &Path, depth: usize) -> Vec<(String, PathBuf)> {
    let mut exes = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else { return exes };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        let path = entry.path();
        if file_type.is_dir() && depth > 0 {
            exes.extend(find_exes(&path, depth - 1));
        } else if file_type.is_file()
            && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            exes.push((entry.file_name().to_string_lossy().to_lowercase(), path));
        }
    }
    exes
}

// ---- Steam ----------------------------------------------------------------------------

fn steam_games() -> Vec<(String, PathBuf)> {
    let steam_dir = steam_install_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_STEAM_DIR));
    let mut libraries = vec![steam_dir.clone()];
    if let Ok(vdf) = fs::read_to_string(steam_dir.join("steamapps").join("libraryfolders.vdf")) {
        libraries.extend(vdf_values(&vdf, "path").map(PathBuf::from));
    }
    libraries.sort();
    libraries.dedup();

    let mut games = Vec::new();
    for library in libraries {
        let steamapps = library.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else { continue };
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_lowercase();
            if !(file_name.starts_with("appmanifest_") && file_name.ends_with(".acf")) {
                continue;
            }
            let Ok(manifest) = fs::read_to_string(entry.path()) else { continue };
            let name = vdf_values(&manifest, "name").next();
            let install = vdf_values(&manifest, "installdir").next();
            if let (Some(name), Some(install)) = (name, install) {
                games.push((name, steamapps.join("common").join(install)));
            }
        }
    }
    games
}

/// Values of `"key"  "value"` lines in Valve's KeyValues text format, unescaped.
fn vdf_values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = String> + 'a {
    text.lines().filter_map(move |line| {
        let mut parts = line.trim().split('"').filter(|p| !p.trim().is_empty());
        let found_key = parts.next()?;
        let value = parts.next()?;
        found_key.eq_ignore_ascii_case(key).then(|| value.replace(r"\\", r"\"))
    })
}

fn steam_install_dir() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let subkey = wide(r"Software\Valve\Steam");
    let value = wide("SteamPath");
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: buffers are valid for the given byte length and NUL-terminated inputs.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
}

// ---- Epic -----------------------------------------------------------------------------

fn epic_games() -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(EPIC_MANIFEST_DIR) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("item")))
        .filter_map(|e| parse_epic_manifest(&fs::read_to_string(e.path()).ok()?))
        .collect()
}

fn parse_epic_manifest(json: &str) -> Option<(String, PathBuf)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let title = value.get("DisplayName")?.as_str()?.to_owned();
    let install = value.get("InstallLocation")?.as_str()?;
    Some((title, PathBuf::from(install)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_vdf_values() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;
        let paths: Vec<String> = vdf_values(vdf, "path").collect();
        assert_eq!(paths, [r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]);
    }

    #[test]
    fn reads_app_manifest() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"installdir\"\t\t\"ELDEN RING\"\n}";
        assert_eq!(vdf_values(acf, "name").next().as_deref(), Some("ELDEN RING"));
        assert_eq!(vdf_values(acf, "installdir").next().as_deref(), Some("ELDEN RING"));
    }

    #[test]
    fn reads_epic_manifest() {
        let json = r#"{"DisplayName":"Some Game","InstallLocation":"D:\\Epic\\SomeGame","LaunchExecutable":"SomeGame.exe"}"#;
        let (title, dir) = parse_epic_manifest(json).unwrap();
        assert_eq!(title, "Some Game");
        assert_eq!(dir, PathBuf::from(r"D:\Epic\SomeGame"));
    }
}
