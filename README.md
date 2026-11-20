# Shadersweep

Manage shader caches for individual games. Shadersweep is a small Windows app (Rust +
[GPUI](https://www.gpui.rs/)) for NVIDIA's DirectX shader cache: see what each game is using and
clear just the ones you choose.

## Download

| Version | Download | Description |
|---|---|---|
| Installer | [shadersweep-setup.exe](https://github.com/etherloper/shadersweep/releases/latest/download/shadersweep-setup.exe) | Installs for your account (no admin needed), adds a Start menu shortcut and an uninstaller. |
| Portable | [shadersweep.exe](https://github.com/etherloper/shadersweep/releases/latest/download/shadersweep.exe) | A single exe you can run from any folder. Nothing is installed. |

Both always point to the latest release and update themselves from within the app. Requires
Windows 10 or 11 (64-bit).

## Features

- Groups every `.nvph` cache file by the program that created it.
- An overview splits the cache into installed games, uninstalled games and other apps, and
  ranks the largest caches.
- Shows real game names and icons (`pbz.exe` → *Phantom Blade Zero*) from your Steam and Epic
  libraries, including Steam games you've since uninstalled.
- Shows where each game's exe lives, with a button to open its folder.
- Sort by name, last updated, file count or size. Select every uninstalled game's cache in one
  click.
- Deletes the cache for one game, several checked games, or straight from a row's trash button,
  always after a confirmation.
- Warns when a game is running. Files it has open are skipped and reported, never forced.
- Only ever deletes `.nvph` files directly inside the cache folder.
- Light, dark (true black) or follow-Windows themes, chosen in Settings.
- Checks GitHub for new versions and installs them in place, after verifying the download's
  checksum, when you click Install.
