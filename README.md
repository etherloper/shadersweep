# Shadersweep

A small Windows app (Rust + [GPUI](https://www.gpui.rs/)) that shows what is inside NVIDIA's
DirectX shader cache (`%LOCALAPPDATA%\NVIDIA\DXCache`), grouped by game, and lets you delete
the cache for individual games instead of wiping the whole folder.

## Download

| Version | Download | Description |
|---|---|---|
| Installer | [shadersweep-setup.exe](https://github.com/etherloper/shadersweep/releases/latest/download/shadersweep-setup.exe) | Installs for your account (no admin needed), adds a Start menu shortcut and an uninstaller. |
| Portable | [shadersweep.exe](https://github.com/etherloper/shadersweep/releases/latest/download/shadersweep.exe) | A single exe you can run from any folder. Nothing is installed. |

Both always point to the latest release and update themselves from within the app. Requires
Windows 10 or 11 (64-bit).

## Features

- Groups every `.nvph` cache file by the program that created it.
- Overview cards and a usage bar show where the space goes at a glance.
- Shows friendly titles and real icons for installed Steam and Epic games
  (`pbz.exe` → *Phantom Blade Zero*), and icons for running apps.
- Sort by name, last updated, file count or size. Filter to caches not updated recently and
  select them all in one click.
- Deletes the cache for one game, several checked games, or straight from a row's trash button,
  always after a confirmation.
- Warns when a game is running. Files it has open are skipped and reported, never forced.
- Only ever deletes `.nvph` files directly inside the cache folder.
- Light, dark (true black) or follow-Windows themes, chosen in Settings.
- Checks GitHub for new versions and installs them in place, after verifying the download's
  checksum, when you click Install.
