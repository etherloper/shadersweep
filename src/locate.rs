//! Finds where executables live using records Windows keeps of programs this user has
//! launched, so caches from games outside the Steam/Epic libraries (or since uninstalled)
//! can still be named and traced to a folder.
//!
//! Sources, both per-user and readable without elevation:
//! - `MuiCache`: `<exe path>.FriendlyAppName` = the exe's file description.
//! - Program Compatibility Assistant store: value names are exe paths.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey,
    RegEnumKeyExW, RegEnumValueW, RegGetValueW, RegOpenKeyExW,
};

const MUI_CACHE: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache";
const PCA_STORE: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store";
const FRIENDLY_SUFFIX: &str = ".FriendlyAppName";

#[derive(Clone, Debug, PartialEq)]
pub struct SeenExe {
    pub path: PathBuf,
    /// Name Windows recorded for the program (its file description), when useful.
    pub recorded_name: Option<String>,
    pub exists: bool,
}

impl SeenExe {
    /// The best human name this record offers: a launcher's game folder beats the
    /// recorded description, which is sometimes just a launcher ("Embark Game Launcher").
    pub fn display_name(&self) -> Option<String> {
        game_folder_name(&self.path).or_else(|| self.recorded_name.clone())
    }
}

/// Lowercased exe file name → best record for it.
pub type SeenExes = HashMap<String, SeenExe>;

pub fn discover() -> SeenExes {
    let mut candidates: Vec<(PathBuf, Option<String>)> = Vec::new();
    for (name, value) in read_values(MUI_CACHE, true) {
        if let Some(path) = name.strip_suffix(FRIENDLY_SUFFIX) {
            candidates.push((PathBuf::from(path), value));
        }
    }
    for (name, _) in read_values(PCA_STORE, false) {
        candidates.push((PathBuf::from(name), None));
    }
    choose_best(candidates, |path| path.exists())
}

fn choose_best(candidates: Vec<(PathBuf, Option<String>)>, exists: impl Fn(&Path) -> bool) -> SeenExes {
    let mut best = SeenExes::new();
    for (path, recorded) in candidates {
        let is_exe = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else { continue };
        if !is_exe || !path.is_absolute() {
            continue;
        }
        let recorded_name = recorded.filter(|n| is_meaningful_name(n, &file_name));
        let candidate = SeenExe { exists: exists(&path), path, recorded_name };
        match best.get_mut(&file_name) {
            Some(current) if score(current) >= score(&candidate) => {
                // Keep the better path, but don't lose a name only the other record had.
                if current.recorded_name.is_none() && current.path == candidate.path {
                    current.recorded_name = candidate.recorded_name;
                }
            }
            _ => {
                best.insert(file_name, candidate);
            }
        }
    }
    best
}

/// Prefer files still on disk, then records with a name, then deeper paths (a game's real
/// binary under `Binaries\Win64` over a launcher stub at the install root).
fn score(exe: &SeenExe) -> (bool, bool, usize) {
    (exe.exists, exe.recorded_name.is_some(), exe.path.components().count())
}

fn is_meaningful_name(name: &str, file_name: &str) -> bool {
    let name = name.trim();
    let lower = name.to_lowercase();
    !name.is_empty() && lower != file_name && !lower.ends_with(".exe")
}

/// The game's own folder under a known launcher library, e.g.
/// `D:\SteamLibrary\steamapps\common\Arc Raiders\...` → `Arc Raiders`.
pub fn game_folder_name(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let lower: Vec<String> = parts.iter().map(|p| p.to_lowercase()).collect();
    let after = |markers: &[&str]| -> Option<String> {
        lower.windows(markers.len()).position(|w| w == markers).and_then(|ix| {
            let folder = parts.get(ix + markers.len())?;
            // The exe itself sitting directly in the library folder is not a game folder.
            (ix + markers.len() < parts.len() - 1).then(|| folder.clone())
        })
    };
    after(&["steamapps", "common"])
        .or_else(|| after(&["epic games"]))
        .or_else(|| after(&["xboxgames"]))
        .or_else(|| after(&["gog galaxy", "games"]))
        .or_else(|| after(&["gog games"]))
}

/// The launcher a path belongs to, judged by its library folder.
pub fn launcher_name(path: &Path) -> Option<&'static str> {
    let lower = path.to_string_lossy().to_lowercase();
    [
        (r"\steamapps\", "Steam"),
        (r"\epic games\", "Epic Games"),
        (r"\xboxgames\", "Xbox"),
        (r"\gog galaxy\", "GOG"),
        (r"\gog games\", "GOG"),
    ]
    .into_iter()
    .find(|(marker, _)| lower.contains(marker))
    .map(|(_, name)| name)
}

/// Game install folders paired with the launcher that installed them.
pub type LauncherInstalls = Vec<(PathBuf, &'static str)>;

/// Windows' lists of installed programs (per machine, 32-bit view, and per user).
const UNINSTALL_KEYS: [(HKEY, &str); 3] = [
    (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
];

/// Launchers that install each game in its own folder, recognised by their uninstaller:
/// (text in the entry's UninstallString, launcher name).
const LAUNCHER_UNINSTALLERS: [(&str, &str); 1] = [("blizzard uninstaller", "Battle.net")];

/// Games installed by launchers without a shared library folder (Battle.net), taken from
/// Windows' list of installed programs. The launchers' own entries are left out.
pub fn launcher_installs() -> LauncherInstalls {
    let mut installs = LauncherInstalls::new();
    for (root, path) in UNINSTALL_KEYS {
        for entry in subkey_names(root, path) {
            let key = format!(r"{path}\{entry}");
            let Some(uninstall) = read_string(root, &key, "UninstallString") else { continue };
            let uninstall = uninstall.to_lowercase();
            let Some(&(_, launcher)) = LAUNCHER_UNINSTALLERS.iter().find(|(marker, _)| uninstall.contains(marker)) else {
                continue;
            };
            if read_string(root, &key, "DisplayName").is_some_and(|name| name.eq_ignore_ascii_case(launcher)) {
                continue;
            }
            let Some(location) = read_string(root, &key, "InstallLocation") else { continue };
            let dir = location.trim().trim_matches('"').trim_end_matches(['\\', '/']);
            if !dir.is_empty() {
                installs.push((PathBuf::from(dir), launcher));
            }
        }
    }
    installs
}

/// The launcher install (folder, launcher) that contains `exe`.
pub fn installed_by<'a>(installs: &'a LauncherInstalls, exe: &Path) -> Option<&'a (PathBuf, &'static str)> {
    let exe = exe.to_string_lossy().to_lowercase();
    installs.iter().find(|(dir, _)| exe.starts_with(&format!(r"{}\", dir.to_string_lossy().to_lowercase())))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn subkey_names(root: HKEY, path: &str) -> Vec<String> {
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: valid NUL-terminated key path; the handle is closed below.
    if unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    let mut name = [0u16; 256];
    for index in 0.. {
        let mut len = name.len() as u32;
        // SAFETY: `name` holds `len` characters; the optional out-parameters are null.
        let status = unsafe {
            RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut len, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut())
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&name[..len as usize]));
    }
    // SAFETY: `key` was opened above.
    unsafe { RegCloseKey(key) };
    names
}

fn read_string(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: `buf` is valid for `bytes`; the key path and value name are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            root,
            wide(subkey).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let chars = (bytes as usize / 2).min(buf.len());
    let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// Value names (and string data, when asked) under an HKCU key. Missing keys yield nothing.
fn read_values(subkey: &str, want_strings: bool) -> Vec<(String, Option<String>)> {
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: valid NUL-terminated key path; the handle is closed below.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut values = Vec::new();
    let mut name = vec![0u16; 32_768];
    let mut data = vec![0u16; 4_096];
    for index in 0.. {
        let mut name_len = name.len() as u32;
        let mut data_bytes = (data.len() * 2) as u32;
        let mut kind = 0u32;
        // SAFETY: buffers and their lengths match; lengths are updated by the call.
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                ptr::null(),
                &mut kind,
                if want_strings { data.as_mut_ptr().cast() } else { ptr::null_mut() },
                if want_strings { &mut data_bytes } else { ptr::null_mut() },
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            continue; // e.g. a value larger than our buffer; not one we need
        }
        let value_name = String::from_utf16_lossy(&name[..name_len as usize]);
        let value = (want_strings && kind == REG_SZ).then(|| {
            let chars = (data_bytes as usize / 2).min(data.len());
            let end = data[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
            String::from_utf16_lossy(&data[..end])
        });
        values.push((value_name, value));
    }
    // SAFETY: `key` was opened above.
    unsafe { RegCloseKey(key) };
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_games_from_launcher_folders() {
        let name = |p: &str| game_folder_name(Path::new(p));
        assert_eq!(
            name(r"D:\SteamLibrary\steamapps\common\Arc Raiders\PioneerGame\Binaries\Win64\PioneerGame.exe").as_deref(),
            Some("Arc Raiders")
        );
        assert_eq!(name(r"C:\Program Files\Epic Games\Fortnite\FortniteClient.exe").as_deref(), Some("Fortnite"));
        assert_eq!(name(r"C:\XboxGames\Starfield\Content\Starfield.exe").as_deref(), Some("Starfield"));
        assert_eq!(name(r"C:\Program Files (x86)\GOG Galaxy\Games\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe").as_deref(), Some("Cyberpunk 2077"));
        assert_eq!(name(r"C:\Tools\vidi\vidi.exe"), None);
        assert_eq!(name(r"C:\Program Files\Epic Games\launcher.exe"), None);
    }

    #[test]
    fn matches_exes_to_launcher_install_folders() {
        let installs: LauncherInstalls = vec![(PathBuf::from(r"D:\Battle.net\World of Warcraft"), "Battle.net")];
        let wow = Path::new(r"D:\Battle.net\World of Warcraft\_retail_\Wow.exe");
        assert_eq!(installed_by(&installs, wow).map(|(_, launcher)| *launcher), Some("Battle.net"));
        // A sibling folder that merely starts with the same name is not inside it.
        assert!(installed_by(&installs, Path::new(r"D:\Battle.net\World of Warcraft Classic\Wow.exe")).is_none());
    }

    #[test]
    fn recognises_launcher_folders() {
        let launcher = |p: &str| launcher_name(Path::new(p));
        assert_eq!(launcher(r"D:\SteamLibrary\steamapps\common\Arc Raiders\PioneerGame.exe"), Some("Steam"));
        assert_eq!(launcher(r"C:\XboxGames\Starfield\Content\Starfield.exe"), Some("Xbox"));
        assert_eq!(launcher(r"C:\Tools\vidi\vidi.exe"), None);
    }

    #[test]
    fn prefers_existing_named_and_deeper_records() {
        let stub = PathBuf::from(r"D:\SteamLibrary\steamapps\common\Arc Raiders\PioneerGame.exe");
        let real = PathBuf::from(r"D:\SteamLibrary\steamapps\common\Arc Raiders\PioneerGame\Binaries\Win64\PioneerGame.exe");
        let seen = choose_best(
            vec![
                (stub.clone(), Some("Embark Game Launcher".into())),
                (real.clone(), Some("ARC Raiders".into())),
                (PathBuf::from(r"C:\Windows\explorer.exe"), Some("Windows Explorer".into())),
                (PathBuf::from("relative.exe"), None),
                (PathBuf::from(r"C:\notes.txt"), None),
            ],
            |_| false,
        );
        let pioneer = &seen["pioneergame.exe"];
        assert_eq!(pioneer.path, real);
        assert_eq!(pioneer.display_name().as_deref(), Some("Arc Raiders"));
        assert!(!seen.contains_key("relative.exe"));
        assert!(!seen.contains_key("notes.txt"));

        let on_disk = choose_best(vec![(real.clone(), None), (stub.clone(), None)], |p| p == stub);
        assert_eq!(on_disk["pioneergame.exe"].path, stub, "a file that still exists wins");
    }

    #[test]
    fn ignores_names_that_just_repeat_the_exe() {
        let seen = choose_best(
            vec![(PathBuf::from(r"C:\Games\Demo\Ignite the Depths Demo.exe"), Some("Ignite the Depths Demo.exe".into()))],
            |_| true,
        );
        assert_eq!(seen["ignite the depths demo.exe"].recorded_name, None);
    }
}
