mod components;
mod views;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    Context, FocusHandle, KeyDownEvent, RenderImage, ScrollStrategy, UniformListScrollHandle, Window, div,
    prelude::*, rgb,
};

use crate::cache::{self, AppCache, CacheFile, DeleteReport, ScanResult};
use crate::format::{self, plural};
use crate::library::{self, GameLibrary, Launcher};
use crate::locate::{self, LauncherInstalls, SeenExes};
use crate::steam_appinfo::{self, SteamCatalog, SteamCatalogEntry};
use crate::native;
use crate::processes::{self, RunningProcesses};
use crate::settings::{self, Settings};
use crate::theme::{self, ThemeMode, p};
use crate::updater::{self, Release};

const SMALL_CACHE_BYTES: u64 = 1024 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(3);
const NOTICE_DURATION: Duration = Duration::from_secs(3);
/// Toasts with a button stay up longer so there is time to reach it.
const ACTION_NOTICE_DURATION: Duration = Duration::from_secs(12);
const AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const DOWNLOAD_PROGRESS_REFRESH: Duration = Duration::from_millis(150);
const ICON_PIXELS: u32 = 64;

enum ScanState {
    Scanning,
    Ready(ScanResult),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum NoticeKind {
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
enum NoticeAction {
    OpenSettings,
}

struct Notice {
    kind: NoticeKind,
    text: String,
    action: Option<(NoticeAction, &'static str)>,
    /// Distinguishes successive toasts so each one gets its own entrance animation.
    id: usize,
}

enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    /// Installed; relaunching `launch` starts the new version.
    Ready { version: String, launch: PathBuf },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum SortKey {
    Name,
    Updated,
    Files,
    Size,
}

impl SortKey {
    /// Names read best A→Z; every other column is most useful largest/newest first.
    fn default_descending(self) -> bool {
        self != SortKey::Name
    }
}

/// The overview's split of the cache. The order is fixed and matches `Palette::groups`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CacheGroup {
    InstalledGames = 0,
    Uninstalled = 1,
    OtherApps = 2,
}

impl CacheGroup {
    const ALL: [CacheGroup; 3] = [CacheGroup::InstalledGames, CacheGroup::Uninstalled, CacheGroup::OtherApps];
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Overview,
    All,
    Settings,
}

pub struct MainView {
    cache_dir: Option<PathBuf>,
    scan: ScanState,
    /// A rescan is running while the previous result stays on screen.
    refreshing: bool,
    games: GameLibrary,
    /// Executables Windows has recorded this user launching, for apps outside the libraries.
    seen_exes: SeenExes,
    /// Every Steam app the account has seen, keyed by install folder, for uninstalled games.
    steam_catalog: SteamCatalog,
    /// Launcher name (as in `source_of`) → that launcher's own icon, where it is installed.
    launcher_icons: HashMap<&'static str, Arc<RenderImage>>,
    /// Games installed by launchers without a shared library folder (Battle.net).
    launcher_installs: LauncherInstalls,
    running: RunningProcesses,
    /// `None` means loading was attempted and failed; the letter tile is shown instead.
    icons: HashMap<String, Option<Arc<RenderImage>>>,
    settings: Settings,
    /// The tab Settings was opened from; Esc returns there.
    previous_view: View,
    /// Theme last pushed to the native title bar, to avoid redundant DWM calls.
    title_bar_dark: Option<bool>,
    focused: Option<String>,
    /// The app whose cache-file list is expanded in the info panel.
    files_expanded_for: Option<String>,
    marked: BTreeSet<String>,
    view: View,
    sort: SortKey,
    sort_descending: bool,
    confirm: Option<Vec<String>>,
    deleting: bool,
    notice: Option<Notice>,
    next_notice_id: usize,
    update: UpdateState,
    /// Bytes of the update downloaded so far, written by the download thread.
    update_progress: Arc<AtomicU64>,
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
}

impl MainView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        cx.observe_window_appearance(window, |_, _, cx| cx.notify()).detach();

        let (settings, settings_problem) = settings::load();
        let mut view = Self {
            cache_dir: cache::default_cache_dir(),
            scan: ScanState::Scanning,
            refreshing: false,
            games: GameLibrary::new(),
            seen_exes: SeenExes::new(),
            steam_catalog: SteamCatalog::new(),
            launcher_icons: HashMap::new(),
            launcher_installs: LauncherInstalls::new(),
            running: RunningProcesses::new(),
            icons: HashMap::new(),
            settings,
            previous_view: View::All,
            title_bar_dark: None,
            focused: None,
            files_expanded_for: None,
            marked: BTreeSet::new(),
            view: View::All,
            sort: SortKey::Size,
            sort_descending: true,
            confirm: None,
            deleting: false,
            notice: None,
            next_notice_id: 0,
            update: UpdateState::Idle,
            update_progress: Arc::new(AtomicU64::new(0)),
            focus_handle,
            list_scroll: UniformListScrollHandle::new(),
        };
        if let Some(problem) = settings_problem {
            view.show_notice(NoticeKind::Warning, problem, cx);
        }
        view.rescan(cx);
        view.load_games(cx);
        view.poll_running_processes(cx);
        if view.settings.auto_check_updates && view.update_check_due() {
            view.check_for_updates(false, cx);
        }
        view
    }

    // ---- updates ----------------------------------------------------------------------

    fn update_check_due(&self) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.settings
            .last_update_check
            .is_none_or(|last| now.saturating_sub(last) >= AUTO_UPDATE_CHECK_INTERVAL.as_secs())
    }

    /// `manual` checks report every outcome; automatic ones stay quiet unless there is an
    /// update, since being offline is not worth interrupting anyone over.
    fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.update, UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Ready { .. }) {
            return;
        }
        self.update = UpdateState::Checking;
        cx.notify();
        let task = cx.background_executor().spawn(async { updater::check() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                match result {
                    Ok(Some(release)) => {
                        this.update_settings(cx, |s| s.last_update_check = Some(now));
                        if this.view != View::Settings {
                            this.show_notice_with_action(
                                NoticeKind::Success,
                                format!("Shadersweep {} is available", release.version),
                                Some((NoticeAction::OpenSettings, "View")),
                                cx,
                            );
                        }
                        this.update = UpdateState::Available(release);
                    }
                    Ok(None) => {
                        this.update_settings(cx, |s| s.last_update_check = Some(now));
                        this.update = UpdateState::UpToDate;
                    }
                    Err(err) => {
                        this.update = if manual { UpdateState::Failed(err) } else { UpdateState::Idle };
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available(release) = &self.update else { return };
        let release = release.clone();
        self.update_progress.store(0, Ordering::Relaxed);
        self.update = UpdateState::Downloading(release.clone());
        cx.notify();

        let progress = self.update_progress.clone();
        let task = cx.background_executor().spawn({
            let release = release.clone();
            async move { updater::install(&release, progress) }
        });
        // Redraw the progress bar while the download runs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DOWNLOAD_PROGRESS_REFRESH).await;
                let downloading = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.update, UpdateState::Downloading(_))
                });
                if !matches!(downloading, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.update = match result {
                    Ok(launch) => UpdateState::Ready { version: release.version.clone(), launch },
                    Err(err) => UpdateState::Failed(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn restart_into_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Ready { launch, .. } = &self.update else { return };
        match std::process::Command::new(launch).spawn() {
            Ok(_) => cx.quit(),
            Err(err) => {
                let text = format!("Couldn't restart: {err}. Close and reopen Shadersweep to finish updating.");
                self.show_notice(NoticeKind::Error, text, cx);
            }
        }
    }

    fn run_notice_action(&mut self, action: NoticeAction, cx: &mut Context<Self>) {
        match action {
            NoticeAction::OpenSettings => self.show_view(View::Settings, cx),
        }
        self.notice = None;
        cx.notify();
    }

    /// Switches tabs, remembering where Settings was opened from so Esc can go back.
    fn show_view(&mut self, view: View, cx: &mut Context<Self>) {
        if view == View::Settings && self.view != View::Settings {
            self.previous_view = self.view;
        }
        self.view = view;
        cx.notify();
    }

    // ---- background work --------------------------------------------------------------

    fn rescan(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.cache_dir.clone() else {
            self.scan = ScanState::Failed("The LOCALAPPDATA environment variable is not set.".into());
            return;
        };
        match self.scan {
            ScanState::Ready(_) => self.refreshing = true,
            _ => self.scan = ScanState::Scanning,
        }
        cx.notify();
        let task = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { cache::scan(&dir) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                this.scan = match result {
                    Ok(scan) => ScanState::Ready(scan),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => ScanState::Failed(
                        format!("No NVIDIA shader cache found at {}.", dir.display()),
                    ),
                    Err(err) => ScanState::Failed(format!("Could not read {}: {err}", dir.display())),
                };
                this.prune_selection();
                this.load_missing_icons(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_games(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async {
            // A missing or unreadable appinfo.vdf only costs names and icons for
            // uninstalled Steam games, so it is not worth interrupting anyone over.
            let steam_catalog = steam_appinfo::load_catalog(&library::steam_root()).unwrap_or_default();
            let seen_exes = locate::discover();
            let launcher_icons = load_launcher_icons(&seen_exes);
            let launcher_installs = locate::launcher_installs();
            (library::discover_games(), seen_exes, steam_catalog, launcher_icons, launcher_installs)
        });
        cx.spawn(async move |this, cx| {
            let (games, seen_exes, steam_catalog, launcher_icons, launcher_installs) = task.await;
            this.update(cx, |this, cx| {
                this.games = games;
                this.seen_exes = seen_exes;
                this.steam_catalog = steam_catalog;
                this.launcher_icons = launcher_icons;
                this.launcher_installs = launcher_installs;
                this.load_missing_icons(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn poll_running_processes(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let running = cx
                    .background_executor()
                    .spawn(async { processes::running_processes() })
                    .await;
                let still_open = this.update(cx, |this, cx| {
                    if this.running != running {
                        this.running = running;
                        this.load_missing_icons(cx);
                        cx.notify();
                    }
                });
                if still_open.is_err() {
                    break;
                }
                cx.background_executor().timer(PROCESS_POLL_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Loads icons for cached apps whose executable location became known (from the game
    /// library or a running process) and that have not been tried yet.
    fn load_missing_icons(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = self.ready_scan() else { return };
        // The exe's own icon is preferred; Steam's cached artwork covers uninstalled games.
        let wanted: Vec<(String, Option<PathBuf>, Option<PathBuf>)> = scan
            .apps
            .iter()
            .filter(|app| !self.icons.contains_key(&app.key))
            .map(|app| {
                let steam_icon = self.steam_entry(&app.key).and_then(|entry| entry.icon.clone());
                (app.key.clone(), self.exe_path(&app.key), steam_icon)
            })
            .filter(|(_, exe, steam_icon)| exe.is_some() || steam_icon.is_some())
            .collect();
        if wanted.is_empty() {
            return;
        }
        for (key, _, _) in &wanted {
            self.icons.insert(key.clone(), None);
        }
        let task = cx.background_executor().spawn(async move {
            wanted
                .into_iter()
                .map(|(key, exe, steam_icon)| {
                    let icon = exe
                        .and_then(|path| native::exe_icon(&path, ICON_PIXELS))
                        .or_else(|| steam_icon.and_then(|path| native::image_file_icon(&path)));
                    (key, icon)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            this.update(cx, |this, cx| {
                this.icons.extend(loaded);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(keys) = self.confirm.take() else { return };
        let ScanState::Ready(scan) = &self.scan else { return };
        let files: Vec<CacheFile> = scan
            .apps
            .iter()
            .filter(|app| keys.contains(&app.key))
            .flat_map(|app| app.files.iter().cloned())
            .collect();
        let dir = scan.dir.clone();
        self.deleting = true;
        self.notice = None;
        cx.notify();

        let task = cx.background_executor().spawn(async move { cache::delete_files(&dir, &files) });
        cx.spawn(async move |this, cx| {
            let report = task.await;
            this.update(cx, |this, cx| {
                this.deleting = false;
                this.marked.retain(|key| !keys.contains(key));
                let (kind, text) = notice_for(&report);
                this.show_notice(kind, text, cx);
                this.rescan(cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_notice(&mut self, kind: NoticeKind, text: String, cx: &mut Context<Self>) {
        self.show_notice_with_action(kind, text, None, cx);
    }

    fn show_notice_with_action(
        &mut self,
        kind: NoticeKind,
        text: String,
        action: Option<(NoticeAction, &'static str)>,
        cx: &mut Context<Self>,
    ) {
        self.next_notice_id += 1;
        let id = self.next_notice_id;
        let duration = if action.is_some() { ACTION_NOTICE_DURATION } else { NOTICE_DURATION };
        self.notice = Some(Notice { kind, text, action, id });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |this, cx| {
                if this.notice.as_ref().is_some_and(|n| n.id == id) {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- settings ---------------------------------------------------------------------

    fn update_settings(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        let mut next = self.settings.clone();
        change(&mut next);
        if next == self.settings {
            return;
        }
        self.settings = next;
        if let Err(err) = settings::save(&self.settings) {
            self.show_notice(NoticeKind::Error, format!("Couldn't save settings: {err}"), cx);
        }
        cx.notify();
    }

    fn set_theme(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.theme = mode);
    }

    /// Applies the chosen theme before anything is drawn, including the native title bar.
    fn apply_theme(&mut self, window: &Window) {
        let dark = self.settings.theme.is_dark(window.appearance());
        theme::set_dark(dark);
        if self.title_bar_dark != Some(dark) {
            native::style_title_bar(window, p().bg, dark);
            self.title_bar_dark = Some(dark);
        }
    }

    // ---- state helpers ----------------------------------------------------------------

    fn ready_scan(&self) -> Option<&ScanResult> {
        match &self.scan {
            ScanState::Ready(scan) => Some(scan),
            _ => None,
        }
    }

    fn all_apps(&self) -> &[AppCache] {
        self.ready_scan().map_or(&[], |scan| scan.apps.as_slice())
    }

    fn visible_apps(&self) -> Vec<&AppCache> {
        let mut apps: Vec<&AppCache> = self
            .all_apps()
            .iter()
            .filter(|app| !self.settings.hide_small || app.total_size >= SMALL_CACHE_BYTES)
            .collect();
        apps.sort_by(|a, b| {
            let order = match self.sort {
                SortKey::Name => self
                    .display_name(a)
                    .to_lowercase()
                    .cmp(&self.display_name(b).to_lowercase()),
                SortKey::Updated => a.last_modified.cmp(&b.last_modified),
                SortKey::Files => a.files.len().cmp(&b.files.len()),
                SortKey::Size => a.total_size.cmp(&b.total_size),
            };
            let order = if self.sort_descending { order.reverse() } else { order };
            order.then_with(|| b.total_size.cmp(&a.total_size))
        });
        apps
    }

    fn app(&self, key: &str) -> Option<&AppCache> {
        self.all_apps().iter().find(|app| app.key == key)
    }

    /// An executable that exists on disk, for loading icons.
    fn exe_path(&self, key: &str) -> Option<PathBuf> {
        self.exe_location(key).filter(|(_, exists)| *exists).map(|(path, _)| path)
    }

    /// Where an app's executable is (or was last seen), and whether it is still there.
    fn exe_location(&self, key: &str) -> Option<(PathBuf, bool)> {
        if let Some(game) = self.games.get(key) {
            return Some((game.exe_path.clone(), true));
        }
        if let Some(Some(path)) = self.running.get(key) {
            return Some((path.clone(), true));
        }
        self.seen_exes.get(key).map(|seen| (seen.path.clone(), seen.exists))
    }

    /// Which overview group an app belongs to. Only games count as uninstalled: other apps
    /// often move between versioned folders on update (`app-1.0.9…`), leaving an old path
    /// behind while still installed.
    fn group_of(&self, app: &AppCache) -> CacheGroup {
        if !self.is_game(&app.key) {
            CacheGroup::OtherApps
        } else if matches!(self.exe_location(&app.key), Some((_, false))) {
            CacheGroup::Uninstalled
        } else {
            CacheGroup::InstalledGames
        }
    }

    fn is_game(&self, key: &str) -> bool {
        self.games.contains_key(key)
            || self.steam_entry(key).is_some()
            || self.installed_by(key).is_some()
            || self.seen_exes.get(key).is_some_and(|seen| locate::game_folder_name(&seen.path).is_some())
    }

    /// Total size and member keys for each group, in `CacheGroup::ALL` order.
    fn group_totals(&self) -> [(CacheGroup, u64, Vec<String>); 3] {
        let mut totals = CacheGroup::ALL.map(|group| (group, 0u64, Vec::new()));
        for app in self.all_apps() {
            let slot = &mut totals[self.group_of(app) as usize];
            slot.1 += app.total_size;
            slot.2.push(app.key.clone());
        }
        totals
    }

    /// Where an app's name and icon come from: a short launcher name for the header badge
    /// (when there is one) and a sentence for the details.
    fn source_of(&self, key: &str) -> (Option<&'static str>, String) {
        if let Some(game) = self.games.get(key) {
            let launcher = match game.launcher {
                Launcher::Steam => "Steam",
                Launcher::Epic => "Epic Games",
            };
            return (Some(launcher), format!("Installed with {launcher}"));
        }
        if self.steam_entry(key).is_some() {
            return (Some("Steam"), "Steam game, no longer installed".into());
        }
        if store_app_name(key).is_some() {
            return (Some("Microsoft Store"), "Microsoft Store app".into());
        }
        if let Some(launcher) = self.installed_by(key) {
            return (Some(launcher), format!("Installed with {launcher}"));
        }
        if let Some(seen) = self.seen_exes.get(key) {
            return (locate::launcher_name(&seen.path), "Windows launch history".into());
        }
        if matches!(self.running.get(key), Some(Some(_))) {
            return (None, "Currently running".into());
        }
        (None, "Unknown".into())
    }

    /// The launcher (e.g. Battle.net) whose install folder holds the app's exe.
    fn installed_by(&self, key: &str) -> Option<&'static str> {
        self.launcher_install(key).map(|(_, launcher)| *launcher)
    }

    /// The launcher install (game folder, launcher) that holds the app's exe.
    fn launcher_install(&self, key: &str) -> Option<&(PathBuf, &'static str)> {
        let (path, _) = self.exe_location(key)?;
        locate::installed_by(&self.launcher_installs, &path)
    }

    /// Steam's record for an app whose last known exe sits in a Steam library folder.
    fn steam_entry(&self, key: &str) -> Option<&SteamCatalogEntry> {
        let seen = self.seen_exes.get(key)?;
        let in_steam_library = seen
            .path
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("steamapps"));
        if !in_steam_library {
            return None;
        }
        let folder = locate::game_folder_name(&seen.path)?;
        self.steam_catalog.get(&folder.to_lowercase())
    }

    fn prune_selection(&mut self) {
        let existing: HashSet<String> = self.all_apps().iter().map(|app| app.key.clone()).collect();
        self.marked.retain(|key| existing.contains(key));
        if self.focused.as_ref().is_some_and(|key| !existing.contains(key)) {
            self.focused = None;
        }
    }

    fn display_name(&self, app: &AppCache) -> String {
        if !app.is_identified() {
            return "Unidentified files".into();
        }
        if let Some(game) = self.games.get(&app.key) {
            return game.title.clone();
        }
        if let Some(entry) = self.steam_entry(&app.key) {
            return entry.name.clone();
        }
        if let Some(name) = self.seen_exes.get(&app.key).and_then(|seen| seen.display_name()) {
            return name;
        }
        store_app_name(&app.key).unwrap_or_else(|| app.key.clone())
    }

    fn is_running(&self, app: &AppCache) -> bool {
        self.running.contains_key(&app.key)
    }

    fn toggle_marked(&mut self, key: &str) {
        if !self.marked.remove(key) {
            self.marked.insert(key.to_owned());
        }
    }

    fn toggle_all_visible(&mut self) {
        let visible: Vec<String> = self.visible_apps().iter().map(|app| app.key.clone()).collect();
        if visible.iter().all(|key| self.marked.contains(key)) {
            for key in &visible {
                self.marked.remove(key);
            }
        } else {
            self.marked.extend(visible);
        }
    }

    fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort = key;
            self.sort_descending = key.default_descending();
        }
    }

    fn focus_app(&mut self, key: &str, cx: &mut Context<Self>) {
        self.focused = Some(key.to_owned());
        if let Some(ix) = self.visible_apps().iter().position(|app| app.key == key) {
            self.list_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn request_delete(&mut self, keys: Vec<String>, cx: &mut Context<Self>) {
        if !keys.is_empty() && !self.deleting {
            self.confirm = Some(keys);
            cx.notify();
        }
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible = self.visible_apps();
        if visible.is_empty() {
            return;
        }
        let current = self
            .focused
            .as_ref()
            .and_then(|key| visible.iter().position(|app| &app.key == key));
        let next = match current {
            Some(ix) => ix.saturating_add_signed(delta).min(visible.len() - 1),
            None => 0,
        };
        self.focused = Some(visible[next].key.clone());
        self.list_scroll.scroll_to_item(next, ScrollStrategy::Center);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.confirm.is_some() {
            match key {
                "escape" => {
                    self.confirm = None;
                    cx.notify();
                }
                "enter" => self.confirm_delete(cx),
                _ => {}
            }
            return;
        }
        if key == "," && event.keystroke.modifiers.control {
            self.show_view(View::Settings, cx);
            return;
        }
        if self.view == View::Settings {
            if key == "escape" {
                self.show_view(self.previous_view, cx);
            } else if key == "f5" {
                self.rescan(cx);
            }
            return;
        }
        match key {
            "down" => self.move_focus(1, cx),
            "up" => self.move_focus(-1, cx),
            "space" => {
                if let Some(focused) = self.focused.clone() {
                    self.toggle_marked(&focused);
                    cx.notify();
                }
            }
            "delete" => {
                let keys = if self.marked.is_empty() {
                    self.focused.iter().cloned().collect()
                } else {
                    self.marked.iter().cloned().collect()
                };
                self.request_delete(keys, cx);
            }
            "escape" if !self.marked.is_empty() => {
                self.marked.clear();
                cx.notify();
            }
            "escape" if self.focused.is_some() => {
                self.focused = None;
                cx.notify();
            }
            "f5" => self.rescan(cx),
            _ => {}
        }
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_theme(window);
        let p = p();
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .font_family("Segoe UI")
            .text_sm()
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .map(|body| match self.view {
                        View::Overview => body.child(self.render_overview(cx)),
                        View::All => body.child(self.render_list(cx)).children(self.render_detail(cx)),
                        View::Settings => body.child(self.render_settings_page(cx)),
                    }),
            )
            .children(self.render_selection_bar(cx))
            .children(self.render_toast(cx))
            .children(self.render_confirm(cx))
    }
}

/// Icons taken from the installed launchers themselves, located through Windows' launch
/// records first and default install folders second. Launchers that aren't found are
/// simply absent, and their name is shown instead.
fn load_launcher_icons(seen_exes: &SeenExes) -> HashMap<&'static str, Arc<RenderImage>> {
    library::launcher_exes()
        .into_iter()
        .filter_map(|(name, exe, defaults)| {
            let path = seen_exes
                .get(exe)
                .filter(|seen| seen.exists)
                .map(|seen| seen.path.clone())
                .or_else(|| defaults.into_iter().find(|path| path.is_file()))?;
            Some((name, native::exe_icon(&path, ICON_PIXELS)?))
        })
        .collect()
}

fn percent(part: u64, whole: u64) -> String {
    let value = part as f64 * 100.0 / whole.max(1) as f64;
    if value > 0.0 && value < 1.0 { "<1%".into() } else { format!("{value:.0}%") }
}

/// Store apps are recorded by package family name (`microsoft.windows.photos_8wekyb3d8bbwe`).
/// Dropping the publisher hash leaves something a person can recognise.
fn store_app_name(key: &str) -> Option<String> {
    if key.ends_with(".exe") {
        return None;
    }
    let (package, publisher) = key.rsplit_once('_')?;
    let is_publisher_hash = publisher.len() == 13 && publisher.bytes().all(|b| b.is_ascii_alphanumeric());
    is_publisher_hash.then(|| package.to_owned())
}

/// One short line for the toast; details beyond the first failure are not useful there.
fn notice_for(report: &DeleteReport) -> (NoticeKind, String) {
    let freed = format!("Freed {} · {} deleted", format::bytes(report.freed), plural(report.deleted, "file", "files"));
    if let Some((_, err)) = report.failed.first() {
        let failed = plural(report.failed.len(), "file", "files");
        return (NoticeKind::Error, format!("{freed} · {failed} couldn't be deleted: {err}"));
    }
    if report.in_use > 0 {
        let skipped = plural(report.in_use, "file", "files");
        return (NoticeKind::Warning, format!("{freed} · {skipped} in use, skipped"));
    }
    (NoticeKind::Success, freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_names_drop_publisher_hash() {
        assert_eq!(
            store_app_name("microsoft.windows.photos_8wekyb3d8bbwe").as_deref(),
            Some("microsoft.windows.photos")
        );
        assert_eq!(store_app_name("t3 code (beta).exe"), None);
        assert_eq!(store_app_name("some_tool"), None);
    }

    #[test]
    fn percentages_round_and_flag_tiny_shares() {
        assert_eq!(percent(50, 100), "50%");
        assert_eq!(percent(1, 1000), "<1%");
        assert_eq!(percent(0, 100), "0%");
    }

    #[test]
    fn toast_text_is_one_short_line() {
        let report = DeleteReport { deleted: 3, freed: 16 * 1024 * 1024, ..Default::default() };
        let (kind, text) = notice_for(&report);
        assert!(kind == NoticeKind::Success);
        assert_eq!(text, "Freed 16.0 MB · 3 files deleted");

        let report = DeleteReport { deleted: 1, freed: 1024, in_use: 2, ..Default::default() };
        let (kind, text) = notice_for(&report);
        assert!(kind == NoticeKind::Warning);
        assert_eq!(text, "Freed 1.0 KB · 1 file deleted · 2 files in use, skipped");
    }
}
