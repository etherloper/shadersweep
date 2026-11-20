//! Rendering for `MainView`. State changes live in `mod.rs`; this file only draws.

use std::time::{Duration, SystemTime};

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, FontWeight, SharedString, Stateful, div,
    ease_out_quint, img, prelude::*, px, relative, rgb, rgba, uniform_list,
};

use super::components::*;
use super::{
    Filter, MainView, NoticeAction, NoticeKind, SMALL_CACHE_BYTES, ScanState, SortKey, UpdateState, percent, store_app_name,
};
use crate::assets::Icon;
use crate::cache::AppCache;
use crate::format::{self, plural};
use crate::settings::{STALE_DAY_CHOICES, settings_path};
use crate::theme::{self, Palette, ThemeMode, p};
use crate::updater;

const ROW_HEIGHT: f32 = 60.0;
const DETAIL_WIDTH: f32 = 380.0;
const SETTINGS_WIDTH: f32 = 400.0;
const CONFIRM_LIST_LIMIT: usize = 6;
const USAGE_SEGMENTS: usize = 5;
const ENTRANCE: Duration = Duration::from_millis(180);

impl MainView {
    fn app_icon(&self, app: &AppCache, size: f32) -> AnyElement {
        match self.icons.get(&app.key) {
            Some(Some(image)) => img(image.clone()).size(px(size)).flex_none().into_any_element(),
            _ => letter_tile(&self.display_name(app), &app.key, size).into_any_element(),
        }
    }

    // ---- header and overview ----------------------------------------------------------

    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = p();
        let path = self.cache_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_5()
            .py_3()
            .child(logo())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Shadersweep"))
                    .child(div().text_xs().text_color(rgb(p.muted)).truncate().child(path)),
            )
            .when(self.refreshing, |header| {
                header.child(div().text_xs().text_color(rgb(p.muted)).child("Refreshing…"))
            })
            .child(
                icon_button("rescan", Some(Icon::Refresh), "Rescan", ButtonStyle::Secondary)
                    .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
            )
            .when_some(self.cache_dir.clone(), |header, dir| {
                header.child(
                    icon_button("open-folder", Some(Icon::Folder), "Open folder", ButtonStyle::Secondary)
                        .on_click(move |_, _, cx| cx.open_url(&dir.to_string_lossy())),
                )
            })
            .child(
                icon_button("open-settings", Some(Icon::Settings), "Settings", ButtonStyle::Secondary)
                    .relative()
                    .when(matches!(self.update, UpdateState::Available(_)), |b| {
                        b.child(
                            div()
                                .absolute()
                                .top(px(-3.0))
                                .right(px(-3.0))
                                .size(px(10.0))
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(p.panel))
                                .bg(rgb(p.accent)),
                        )
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = true;
                        cx.notify();
                    })),
            )
    }

    pub(super) fn render_overview(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = p();
        let scan = self.ready_scan()?;
        let total = scan.total_size.max(1);
        let stale = self.stale_apps();
        let stale_size: u64 = stale.iter().map(|app| app.total_size).sum();
        let recognised = scan.apps.iter().filter(|app| self.games.contains_key(&app.key)).count();
        let largest = scan.apps.first();

        let cards = div()
            .flex()
            .gap_3()
            .child(stat_card(
                Icon::Folder,
                "Total cache",
                format::bytes(scan.total_size),
                format!(
                    "{} across {}",
                    plural(scan.file_count, "file", "files"),
                    plural(scan.apps.len(), "app", "apps")
                ),
                None,
            ))
            .child(stat_card(
                Icon::Info,
                "Largest",
                largest.map(|app| self.display_name(app)).unwrap_or_else(|| "—".into()),
                largest
                    .map(|app| format!("{} · {} of total", format::bytes(app.total_size), percent(app.total_size, total)))
                    .unwrap_or_default(),
                None,
            ))
            .child(stat_card(
                Icon::Clock,
                format!("Not updated in {}+ days", self.settings.stale_days),
                format::bytes(stale_size),
                plural(stale.len(), "app", "apps"),
                (!stale.is_empty()).then(|| {
                    button("select-stale", "Select these", ButtonStyle::Link)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let keys: Vec<String> = this.stale_apps().iter().map(|app| app.key.clone()).collect();
                            this.marked.extend(keys);
                            this.filter = Filter::Stale;
                            this.hide_small = false;
                            cx.notify();
                        }))
                        .into_any_element()
                }),
            ))
            .child(stat_card(
                Icon::Check,
                "Recognised games",
                recognised.to_string(),
                "Matched to your Steam and Epic libraries".into(),
                None,
            ));

        let top: Vec<&AppCache> = scan.apps.iter().take(USAGE_SEGMENTS).collect();
        let other_size = scan.total_size.saturating_sub(top.iter().map(|app| app.total_size).sum());
        let share = |size: u64| (size as f64 / total as f64) as f32;

        let bar = div()
            .flex()
            .gap(px(2.0))
            .h(px(8.0))
            .w_full()
            .children(top.iter().enumerate().map(|(ix, app)| {
                div().h_full().rounded_sm().w(relative(share(app.total_size))).bg(rgb(p.series[ix]))
            }))
            .when(other_size > 0, |bar| {
                bar.child(div().h_full().rounded_sm().flex_1().bg(rgb(p.series_other)))
            });

        let legend = div()
            .flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .children(top.iter().enumerate().map(|(ix, app)| {
                let key = app.key.clone();
                legend_item(("legend", ix), p.series[ix], self.display_name(app), format::bytes(app.total_size))
                    .cursor_pointer()
                    .hover(|item| item.text_color(rgb(p.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let small = this.app(&key).is_some_and(|a| a.total_size < SMALL_CACHE_BYTES);
                        this.hide_small &= !small;
                        this.filter = Filter::All;
                        this.focus_app(&key, cx);
                    }))
            }))
            .when(other_size > 0, |legend| {
                legend.child(legend_item(
                    "legend-other",
                    p.series_other,
                    format!("Everything else ({})", plural(scan.apps.len().saturating_sub(USAGE_SEGMENTS), "app", "apps")),
                    format::bytes(other_size),
                ))
            });

        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .px_5()
                .pb_4()
                .child(cards)
                .child(div().flex().flex_col().gap_2().child(bar).child(legend)),
        )
    }

    // ---- list -------------------------------------------------------------------------

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = p();
        let stale_count = self.stale_apps().len();
        let shown = self.visible_apps().len();
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(p.border))
            .child(segmented([
                segment("filter-all", "All apps".into(), self.filter == Filter::All)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter = Filter::All;
                        cx.notify();
                    }))
                    .into_any_element(),
                segment(
                    "filter-stale",
                    format!("Not updated in {}+ days ({stale_count})", self.settings.stale_days).into(),
                    self.filter == Filter::Stale,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter = Filter::Stale;
                    cx.notify();
                }))
                .into_any_element(),
            ]))
            .child(
                toggle("hide-small", "Hide under 1 MB", self.hide_small)
                    .on_click(cx.listener(|this, _, _, cx| this.set_hide_small(!this.hide_small, cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(format!("Showing {} of {}", shown, self.all_apps().len())),
            )
    }

    pub(super) fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = p();
        let visible_count = self.visible_apps().len();
        let message = match &self.scan {
            ScanState::Scanning => Some("Scanning the shader cache…".to_owned()),
            ScanState::Failed(err) => Some(err.clone()),
            ScanState::Ready(scan) if scan.apps.is_empty() => Some("The shader cache is empty.".into()),
            ScanState::Ready(_) if visible_count == 0 => Some(match (self.filter, self.hide_small) {
                (Filter::Stale, _) => format!("Every cache was updated in the last {} days.", self.settings.stale_days),
                (Filter::All, _) => "Nothing over 1 MB is cached. Turn off \"Hide under 1 MB\" to see everything.".into(),
            }),
            ScanState::Ready(_) => None,
        };

        let body = match message {
            Some(message) => empty_state(Icon::Info, message).into_any_element(),
            None => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(self.render_column_headers(cx))
                .child(
                    uniform_list(
                        "apps",
                        visible_count,
                        cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                            let now = SystemTime::now();
                            let visible = this.visible_apps();
                            let largest = visible.iter().map(|app| app.total_size).max().unwrap_or(1).max(1);
                            range
                                .filter_map(|ix| visible.get(ix).map(|app| this.render_row(ix, app, largest, now, cx)))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.list_scroll.clone())
                    .flex_1(),
                )
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .bg(rgb(p.bg))
            .when(self.ready_scan().is_some(), |list| list.child(self.render_toolbar(cx)))
            .child(body)
            .into_any_element()
    }

    fn render_column_headers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = p();
        let visible = self.visible_apps();
        let marked_visible = visible.iter().filter(|app| self.marked.contains(&app.key)).count();
        let check = match marked_visible {
            0 => CheckState::Unchecked,
            n if n == visible.len() => CheckState::Checked,
            _ => CheckState::Mixed,
        };
        let header = |id: &'static str, label: &'static str, key: SortKey| {
            let active = self.sort == key;
            let arrow = match (active, self.sort_descending) {
                (false, _) => "",
                (true, true) => " ↓",
                (true, false) => " ↑",
            };
            div()
                .id(id)
                .cursor_pointer()
                .when(active, |h| h.text_color(rgb(p.text)))
                .hover(|h| h.text_color(rgb(p.text)))
                .child(format!("{label}{arrow}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_sort(key);
                    cx.notify();
                }))
        };

        div()
            .flex()
            .items_center()
            .gap_3()
            .pl(px(14.0))
            .pr_3()
            .py_2()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(p.muted))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(checkbox("mark-all", check).on_click(cx.listener(|this, _, _, cx| {
                this.toggle_all_visible();
                cx.notify();
            })))
            .child(div().w(px(32.0)))
            .child(div().flex_1().child(header("sort-name", "App", SortKey::Name)))
            .child(div().w(px(120.0)).child(header("sort-updated", "Last updated", SortKey::Updated)))
            .child(div().w(px(48.0)).flex().justify_end().child(header("sort-files", "Files", SortKey::Files)))
            .child(div().w(px(84.0)).flex().justify_end().child(header("sort-size", "Size", SortKey::Size)))
            .child(div().w(px(28.0)))
    }

    fn render_row(
        &self,
        ix: usize,
        app: &AppCache,
        largest: u64,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = p();
        let key = app.key.clone();
        let group: SharedString = format!("row-{ix}").into();
        let focused = self.focused.as_deref() == Some(app.key.as_str());
        let marked = self.marked.contains(&app.key);
        let stale = self.is_stale(app, now);
        let name = self.display_name(app);
        let show_key = app.is_identified() && name != app.key;
        let fraction = (app.total_size as f64 / largest as f64).clamp(0.01, 1.0) as f32;

        div()
            .id(("app-row", ix))
            .group(group.clone())
            .relative()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_3()
            .pl(px(14.0))
            .pr_3()
            .border_b_1()
            .border_color(rgb(p.border))
            .cursor_pointer()
            .when(focused, |row| {
                row.bg(rgb(p.selected))
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(3.0)).bg(rgb(p.accent)))
            })
            .when(!focused, |row| row.hover(|row| row.bg(rgb(p.hover))))
            .on_click(cx.listener({
                let key = key.clone();
                move |this, _, _, cx| {
                    this.focused = Some(key.clone());
                    cx.notify();
                }
            }))
            .child(
                checkbox(("mark", ix), if marked { CheckState::Checked } else { CheckState::Unchecked }).on_click(
                    cx.listener({
                        let key = key.clone();
                        move |this, _, _, cx| {
                            this.toggle_marked(&key);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }),
                ),
            )
            .child(self.app_icon(app, 32.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(name))
                            .when(show_key, |line| {
                                line.child(div().text_xs().text_color(rgb(p.muted)).truncate().child(app.key.clone()))
                            })
                            .when(store_app_name(&app.key).is_some(), |line| line.child(badge("Store app", p.muted)))
                            .when(self.is_running(app), |line| line.child(badge("Running", p.warning))),
                    )
                    .child(
                        div()
                            .h(px(4.0))
                            .w_full()
                            .rounded_full()
                            .bg(rgb(p.raised))
                            .child(div().h_full().w(relative(fraction)).rounded_full().bg(rgb(p.accent))),
                    ),
            )
            .child(
                div()
                    .w(px(120.0))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_color(rgb(if stale { p.warning } else { p.muted }))
                    .when(stale, |cell| cell.child(icon(Icon::Clock, 13.0, p.warning)))
                    .child(format::age(app.last_modified, now)),
            )
            .child(div().w(px(48.0)).text_right().text_color(rgb(p.muted)).child(app.files.len().to_string()))
            .child(
                div()
                    .w(px(84.0))
                    .text_right()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format::bytes(app.total_size)),
            )
            .child(
                square_button(("row-delete", ix), Icon::Trash, 28.0, p.danger)
                    .when(!focused, |b| b.opacity(0.0).group_hover(group, |s| s.opacity(1.0)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.request_delete(vec![key.clone()], cx);
                    })),
            )
    }

    // ---- detail panel -----------------------------------------------------------------

    pub(super) fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = p();
        let panel = div()
            .w(px(DETAIL_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .border_l_1()
            .border_color(rgb(p.border));
        let Some(app) = self.focused.as_deref().and_then(|key| self.app(key)) else {
            return panel.child(empty_state(Icon::Info, "Select an app to see its cache files.".into()));
        };

        let now = SystemTime::now();
        let running = self.is_running(app);
        let locked = app.locked_count();
        let key = app.key.clone();
        let name = self.display_name(app);
        let show_key = app.is_identified() && name != app.key;
        let total = self.ready_scan().map_or(1, |scan| scan.total_size.max(1));
        let largest_file = app.files.first().map_or(1, |f| f.size.max(1));
        let first_file = app.files.first().map(|f| f.path.clone());
        let explanation = if app.is_identified() {
            "The driver rebuilds these shaders the next time the app starts, so the first launch may load slower or stutter briefly."
        } else {
            "These files have a format Shadersweep does not recognise, so it cannot tell which program made them. Deleting them is safe; the driver recreates what it needs."
        };

        panel
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_5()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(self.app_icon(app, 48.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(name))
                                    .when(show_key, |col| {
                                        col.child(div().text_xs().text_color(rgb(p.muted)).truncate().child(app.key.clone()))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(stat("Size", format::bytes(app.total_size)))
                                    .child(stat("Share of cache", percent(app.total_size, total))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(stat("Files", app.files.len().to_string()))
                                    .child(stat("Last updated", format::age(app.last_modified, now))),
                            ),
                    )
                    .when(running || locked > 0, |col| {
                        let text = if running {
                            "This app is running. Close it first, or the files it has open will be skipped.".to_owned()
                        } else {
                            format!("{} in use by another process and will be skipped.", plural(locked, "file is", "files are"))
                        };
                        col.child(callout(text, p.warning))
                    })
                    .child(div().text_sm().text_color(rgb(p.muted)).child(explanation))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                icon_button(
                                    "delete-focused",
                                    Some(Icon::Trash),
                                    if self.deleting {
                                        "Deleting…".to_owned()
                                    } else {
                                        format!("Delete {}", format::bytes(app.total_size))
                                    },
                                    if self.deleting { ButtonStyle::Disabled } else { ButtonStyle::Danger },
                                )
                                .flex_1()
                                .on_click(cx.listener(move |this, _, _, cx| this.request_delete(vec![key.clone()], cx))),
                            )
                            .when_some(first_file, |row, path| {
                                row.child(
                                    icon_button("reveal-files", Some(Icon::Folder), "Show in Explorer", ButtonStyle::Secondary)
                                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px_5()
                    .pt_4()
                    .pb_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(p.muted))
                    .child("Cache files")
                    .child(plural(app.files.len(), "file", "files")),
            )
            .child(
                div()
                    .id("file-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_5()
                    .pb_4()
                    .children(app.files.iter().map(|file| {
                        let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let fraction = (file.size as f64 / largest_file as f64).clamp(0.02, 1.0) as f32;
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .child(div().flex_1().truncate().font_family("Consolas").child(name))
                                    .when(file.locked, |row| row.child(badge("In use", p.warning)))
                                    .child(div().text_color(rgb(p.muted)).child(format::bytes(file.size))),
                            )
                            .child(
                                div()
                                    .h(px(2.0))
                                    .w_full()
                                    .bg(rgb(p.raised))
                                    .child(div().h_full().w(relative(fraction)).bg(rgb(p.border_strong))),
                            )
                    })),
            )
    }

    // ---- overlays ---------------------------------------------------------------------

    pub(super) fn render_selection_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = p();
        if self.marked.is_empty() {
            return None;
        }
        let total: u64 = self.marked.iter().filter_map(|key| self.app(key)).map(|app| app.total_size).sum();
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_5()
                .py_3()
                .bg(rgb(p.panel))
                .border_t_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_1()
                        .child(checkbox("selection-indicator", CheckState::Checked))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(plural(self.marked.len(), "app", "apps")))
                        .child(div().text_color(rgb(p.muted)).child(format!("selected · {}", format::bytes(total)))),
                )
                .child(
                    button("clear-selection", "Clear selection", ButtonStyle::Ghost).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.marked.clear();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    icon_button(
                        "delete-selected",
                        Some(Icon::Trash),
                        format!("Delete selected ({})", format::bytes(total)),
                        if self.deleting { ButtonStyle::Disabled } else { ButtonStyle::Danger },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let keys = this.marked.iter().cloned().collect();
                        this.request_delete(keys, cx);
                    })),
                ),
        )
    }

    /// A compact pill at the bottom centre. Toasts are dark in both themes, so their
    /// status colours come from the dark palette.
    pub(super) fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = p();
        let notice = self.notice.as_ref()?;
        // The update toast only points at Settings; once they are open it has done its job.
        if self.settings_open && notice.action.is_some_and(|(action, _)| action == NoticeAction::OpenSettings) {
            return None;
        }
        let (glyph, tint) = match notice.kind {
            NoticeKind::Success => (Icon::Check, theme::DARK.accent),
            NoticeKind::Warning => (Icon::Alert, theme::DARK.warning),
            NoticeKind::Error => (Icon::Alert, theme::DARK.danger),
        };
        let bottom = if self.marked.is_empty() { 20.0 } else { 76.0 };
        Some(
            div().absolute().left_0().right_0().bottom(px(bottom)).flex().justify_center().child(
                div()
                    .id(("toast", notice.id))
                    .flex()
                    .items_center()
                    .gap_2()
                    .max_w(px(620.0))
                    .h(px(36.0))
                    .pl(px(12.0))
                    .pr(px(4.0))
                    .rounded_full()
                    .bg(rgb(p.toast_bg))
                    .border_1()
                    .border_color(with_alpha(p.toast_text, 0.10))
                    .shadow_lg()
                    .text_sm()
                    .text_color(rgb(p.toast_text))
                    .child(icon(glyph, 16.0, tint))
                    .child(div().truncate().child(notice.text.clone()))
                    .when_some(notice.action, |toast, (action, label)| {
                        toast.child(
                            div()
                                .id("toast-action")
                                .flex_none()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .cursor_pointer()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme::DARK.accent))
                                .hover(|b| b.bg(with_alpha(p.toast_text, 0.12)))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| this.run_notice_action(action, cx))),
                        )
                    })
                    .child(
                        div()
                            .id("dismiss-toast")
                            .size(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|b| b.bg(with_alpha(p.toast_text, 0.12)))
                            .child(icon(Icon::X, 14.0, p.toast_text))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            })),
                    )
                    .with_animation(
                        ("toast-in", notice.id),
                        Animation::new(ENTRANCE).with_easing(ease_out_quint()),
                        |toast, delta| toast.opacity(delta),
                    ),
            ),
        )
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.settings_open {
            return None;
        }
        let p = p();
        let close = cx.listener(|this, _, _, cx| {
            this.settings_open = false;
            cx.notify();
        });

        let themes = [
            (ThemeMode::System, Icon::Monitor, "System"),
            (ThemeMode::Light, Icon::Sun, "Light"),
            (ThemeMode::Dark, Icon::Moon, "Dark"),
        ];
        let theme_cards = div().flex().gap_2().children(themes.into_iter().enumerate().map(|(ix, (mode, glyph, label))| {
            let active = self.settings.theme == mode;
            div()
                .id(("theme", ix))
                .flex_1()
                .flex()
                .flex_col()
                .gap_2()
                .p(px(6.0))
                .rounded_lg()
                .border_2()
                .border_color(rgb(if active { p.accent } else { p.border }))
                .cursor_pointer()
                .when(!active, |card| card.hover(|card| card.border_color(rgb(p.border_strong))))
                .child(theme_preview(mode))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_1()
                        .pb_1()
                        .text_sm()
                        .text_color(rgb(if active { p.text } else { p.muted }))
                        .when(active, |row| row.font_weight(FontWeight::MEDIUM))
                        .child(icon(glyph, 14.0, if active { p.text } else { p.muted }))
                        .child(label),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_theme(mode, cx)))
        }));

        let stale_choices = segmented(STALE_DAY_CHOICES.iter().map(|&days| {
            segment(("stale-days", days as usize), format!("{days} days").into(), self.settings.stale_days == days)
                .on_click(cx.listener(move |this, _, _, cx| this.update_settings(cx, |s| s.stale_days = days)))
                .into_any_element()
        }));

        let hide_small_row = div()
            .id("setting-hide-small")
            .flex()
            .items_center()
            .gap_3()
            .p_3()
            .rounded_md()
            .bg(rgb(p.raised))
            .cursor_pointer()
            .hover(|row| row.bg(rgb(p.hover)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Hide caches under 1 MB"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child("Small caches mostly belong to browsers, launchers and Windows itself."),
                    ),
            )
            .child(switch(self.settings.hide_small))
            .on_click(cx.listener(|this, _, _, cx| this.set_hide_small(!this.settings.hide_small, cx)));

        let shortcuts = [
            ("↑ ↓", "Move between apps"),
            ("Space", "Check or uncheck"),
            ("Delete", "Delete checked or current"),
            ("Esc", "Clear selection or close"),
            ("F5", "Rescan"),
            ("Ctrl ,", "Open settings"),
        ];

        let sheet = div()
            .id("settings-sheet")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(SETTINGS_WIDTH))
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .border_l_1()
            .border_color(rgb(p.border_strong))
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(div().flex_1().text_lg().font_weight(FontWeight::SEMIBOLD).child("Settings"))
                    .child(square_button("close-settings", Icon::X, 32.0, p.muted).on_click(close)),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .p_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(section_label("Appearance"))
                            .child(theme_cards)
                            .child(div().text_xs().text_color(rgb(p.muted)).child(
                                "System follows your Windows app mode. Dark uses a true black background.",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(section_label("Cache"))
                            .child(div().font_weight(FontWeight::MEDIUM).child("Count a cache as not updated after"))
                            .child(div().flex().child(stale_choices))
                            .child(hide_small_row),
                    )
                    .child(self.render_update_section(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(section_label("Keyboard"))
                            .children(shortcuts.into_iter().map(|(keys, action)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_sm()
                                    .child(div().text_color(rgb(p.muted)).child(action))
                                    .child(kbd(keys))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(section_label("About"))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("Shadersweep {}", env!("CARGO_PKG_VERSION"))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child("Shows NVIDIA's DirectX shader cache by game and clears it one game at a time."),
                            )
                            .children(settings_path().map(|path| {
                                div()
                                    .pt_2()
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child(format!("Settings are saved to {}", path.display()))
                            })),
                    ),
            )
            .with_animation(
                "settings-in",
                Animation::new(ENTRANCE).with_easing(ease_out_quint()),
                |sheet, delta| sheet.opacity(delta).right(px(-24.0 * (1.0 - delta))),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("settings-backdrop")
                        .absolute()
                        .inset_0()
                        .bg(rgba(p.backdrop))
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_open = false;
                            cx.notify();
                        })),
                )
                .child(sheet),
        )
    }

    fn render_update_section(&self, cx: &mut Context<Self>) -> Div {
        let p = p();
        let current = updater::current_version();
        let checked = self.settings.last_update_check.map(|secs| {
            let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
            format::age(Some(when), SystemTime::now()).to_lowercase()
        });
        let check_button = |label: &'static str| {
            icon_button("check-updates", Some(Icon::Refresh), label, ButtonStyle::Secondary)
                .on_click(cx.listener(|this, _, _, cx| this.check_for_updates(true, cx)))
        };

        let status: AnyElement = match &self.update {
            UpdateState::Idle | UpdateState::UpToDate => {
                let detail = match (&self.update, &checked) {
                    (UpdateState::UpToDate, _) => "You're on the latest version.".to_owned(),
                    (_, Some(when)) => format!("Last checked {when}."),
                    (_, None) => "Not checked yet.".to_owned(),
                };
                status_row(Icon::Check, p.accent, format!("Shadersweep {current}"), detail)
                    .child(check_button("Check now"))
                    .into_any_element()
            }
            UpdateState::Checking => status_row(Icon::Refresh, p.muted, format!("Shadersweep {current}"), "Checking for updates…".into())
                .into_any_element(),
            UpdateState::Failed(err) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(callout(format!("Update failed: {err}"), p.danger))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(check_button("Try again"))
                        .child(
                            button("open-releases", "Open releases page", ButtonStyle::Ghost)
                                .on_click(|_, _, cx| cx.open_url(&updater::releases_page())),
                        ),
                )
                .into_any_element(),
            UpdateState::Available(release) => {
                let page = release.page_url.clone();
                let notes: String = release.notes.lines().take(4).collect::<Vec<_>>().join("\n");
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(with_alpha(p.accent, 0.6))
                    .bg(with_alpha(p.accent, 0.08))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Version {} is available", release.version)))
                            .child(div().text_xs().text_color(rgb(p.muted)).child(format!("You have {current}."))),
                    )
                    .when(!notes.is_empty(), |card| {
                        card.child(div().text_xs().text_color(rgb(p.muted)).line_clamp(4).child(notes))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                icon_button("install-update", None, "Download and install", ButtonStyle::Primary)
                                    .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                            )
                            .child(
                                button("release-notes", "Release notes", ButtonStyle::Ghost)
                                    .on_click(move |_, _, cx| cx.open_url(&page)),
                            ),
                    )
                    .into_any_element()
            }
            UpdateState::Downloading(release) => {
                let done = self.update_progress.load(std::sync::atomic::Ordering::Relaxed);
                let total = release.exe_size().max(1);
                let fraction = (done as f64 / total as f64).clamp(0.0, 1.0) as f32;
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_weight(FontWeight::MEDIUM).child(format!("Downloading {}…", release.version)))
                    .child(
                        div()
                            .h(px(6.0))
                            .w_full()
                            .rounded_full()
                            .bg(rgb(p.raised))
                            .child(div().h_full().w(relative(fraction)).rounded_full().bg(rgb(p.accent))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child(format!("{} of {}", format::bytes(done), format::bytes(total))),
                    )
                    .into_any_element()
            }
            UpdateState::Ready { version, .. } => status_row(
                Icon::Check,
                p.accent,
                format!("Version {version} is installed"),
                "Restart Shadersweep to start using it.".into(),
            )
            .child(
                icon_button("restart-now", Some(Icon::Refresh), "Restart now", ButtonStyle::Primary)
                    .on_click(cx.listener(|this, _, _, cx| this.restart_into_update(cx))),
            )
            .into_any_element(),
        };

        let auto_row = div()
            .id("setting-auto-update")
            .flex()
            .items_center()
            .gap_3()
            .p_3()
            .rounded_md()
            .bg(rgb(p.raised))
            .cursor_pointer()
            .hover(|row| row.bg(rgb(p.hover)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Check for updates automatically"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child("Looks for a new release on GitHub once a day. Nothing is installed without asking."),
                    ),
            )
            .child(switch(self.settings.auto_check_updates))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_settings(cx, |s| s.auto_check_updates = !s.auto_check_updates)
            }));

        div().flex().flex_col().gap_3().child(section_label("Updates")).child(status).child(auto_row)
    }

    pub(super) fn render_confirm(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = p();
        let keys = self.confirm.as_ref()?;
        let apps: Vec<&AppCache> = keys.iter().filter_map(|key| self.app(key)).collect();
        let total: u64 = apps.iter().map(|app| app.total_size).sum();
        let files: usize = apps.iter().map(|app| app.files.len()).sum();
        let running: Vec<String> = apps.iter().filter(|app| self.is_running(app)).map(|app| self.display_name(app)).collect();
        let hidden = apps.len().saturating_sub(CONFIRM_LIST_LIMIT);

        Some(
            div()
                .id("confirm-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(p.backdrop))
                .occlude()
                .child(
                    div()
                        .w(px(480.0))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .p_6()
                        .rounded_xl()
                        .bg(rgb(p.panel))
                        .border_1()
                        .border_color(rgb(p.border_strong))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .size(px(36.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(with_alpha(p.danger, 0.14))
                                        .child(icon(Icon::Trash, 18.0, p.danger)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(format!("Delete {} of shader cache?", format::bytes(total))),
                                        )
                                        .child(div().text_color(rgb(p.muted)).child(format!(
                                            "{} will be permanently deleted. Shaders are rebuilt the next time each app runs.",
                                            plural(files, "file", "files"),
                                        ))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(p.border))
                                .children(apps.iter().take(CONFIRM_LIST_LIMIT).map(|app| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(p.border))
                                        .child(self.app_icon(app, 24.0))
                                        .child(div().flex_1().truncate().child(self.display_name(app)))
                                        .child(div().text_color(rgb(p.muted)).child(format::bytes(app.total_size)))
                                }))
                                .when(hidden > 0, |list| {
                                    list.child(
                                        div()
                                            .px_3()
                                            .py_2()
                                            .text_color(rgb(p.muted))
                                            .child(format!("and {} more", plural(hidden, "app", "apps"))),
                                    )
                                }),
                        )
                        .when(!running.is_empty(), |dialog| {
                            dialog.child(callout(
                                format!("Still running: {}. Files they have open will be skipped.", running.join(", ")),
                                p.warning,
                            ))
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .items_center()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child(kbd("Enter"))
                                        .child("delete ·")
                                        .child(kbd("Esc"))
                                        .child("cancel"),
                                )
                                .child(button("confirm-cancel", "Cancel", ButtonStyle::Secondary).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirm = None;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    icon_button(
                                        "confirm-delete",
                                        Some(Icon::Trash),
                                        format!("Delete {}", format::bytes(total)),
                                        ButtonStyle::Danger,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
                                ),
                        ),
                ),
        )
    }
}

fn empty_state(glyph: Icon, message: String) -> Div {
    let p = p();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .text_color(rgb(p.muted))
        .child(
            div()
                .size(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(p.raised))
                .child(icon(glyph, 20.0, p.muted)),
        )
        .child(div().max_w(px(360.0)).text_center().child(message))
}

fn status_row(glyph: Icon, tint: u32, title: String, detail: String) -> Div {
    let p = p();
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .size(px(32.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(with_alpha(tint, 0.14))
                .child(icon(glyph, 16.0, tint)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().text_xs().text_color(rgb(p.muted)).child(detail)),
        )
}

fn kbd(keys: &'static str) -> Div {
    let p = p();
    div()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(rgb(p.border_strong))
        .bg(rgb(p.raised))
        .text_xs()
        .text_color(rgb(p.text))
        .font_family("Consolas")
        .child(keys)
}

/// A miniature of the app in a given theme, for the theme picker.
fn theme_preview(mode: ThemeMode) -> Div {
    fn mini(palette: &Palette) -> Div {
        let line = |width: f32, color: u32| div().h(px(4.0)).w(relative(width)).rounded_full().bg(rgb(color));
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(palette.bg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px(px(6.0))
                    .h(px(14.0))
                    .bg(rgb(palette.panel))
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .child(div().size(px(6.0)).rounded_sm().bg(rgb(palette.accent)))
                    .child(line(0.5, palette.border_strong)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .p(px(6.0))
                    .child(line(0.85, palette.accent))
                    .child(line(0.6, palette.border_strong))
                    .child(line(0.4, palette.border_strong)),
            )
    }
    let frame = div().h(px(64.0)).flex().rounded_md().overflow_hidden().border_1().border_color(rgb(p().border));
    match mode {
        ThemeMode::Light => frame.child(mini(&theme::LIGHT)),
        ThemeMode::Dark => frame.child(mini(&theme::DARK)),
        ThemeMode::System => frame.child(mini(&theme::LIGHT)).child(mini(&theme::DARK)),
    }
}
