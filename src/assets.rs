//! Icons compiled into the executable, served to GPUI's `svg()` element.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Refresh,
    Folder,
    Settings,
    Trash,
    Check,
    X,
    Alert,
    Sun,
    Moon,
    Monitor,
    Info,
    Dashboard,
    List,
    Logo,
    Search,
}

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        impl Icon {
            pub fn path(self) -> &'static str {
                match self { $(Icon::$variant => concat!("icons/", $file, ".svg"),)* }
            }
        }

        fn icon_bytes(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("icons/", $file, ".svg") => {
                    Some(include_bytes!(concat!("../assets/icons/", $file, ".svg")))
                })*
                _ => None,
            }
        }

        const ALL_PATHS: &[&str] = &[$(concat!("icons/", $file, ".svg")),*];
    };
}

icons! {
    Refresh => "refresh",
    Folder => "folder",
    Settings => "settings",
    Trash => "trash",
    Check => "check",
    X => "x",
    Alert => "alert",
    Sun => "sun",
    Moon => "moon",
    Monitor => "monitor",
    Info => "info",
    Dashboard => "dashboard",
    List => "list",
    Logo => "logo",
    Search => "search",
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ALL_PATHS
            .iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(*p))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded() {
        for path in ALL_PATHS {
            let bytes = icon_bytes(path).unwrap();
            assert!(bytes.starts_with(b"<svg"), "{path}");
        }
        assert!(icon_bytes("icons/missing.svg").is_none());
    }
}
