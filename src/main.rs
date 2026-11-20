// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod cache;
mod format;
mod http;
mod library;
mod locate;
mod native;
mod processes;
mod settings;
mod theme;
mod ui;
mod updater;

use gpui::{App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

fn main() {
    updater::clean_up_previous_install();
    Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.0), px(820.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(980.0), px(600.0))),
            titlebar: Some(TitlebarOptions {
                title: Some("Shadersweep".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| cx.new(|cx| ui::MainView::new(window, cx)))
            .expect("failed to open window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
