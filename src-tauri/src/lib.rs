//! mythics.gg Logger: the Tauri shell around `mythics-logger-core`.
//!
//! It reads only World of Warcraft's combat log text file. It never reads the
//! game's memory, injects anything, or automates play. It's safe to run
//! alongside the Warcraft Logs uploader: both only read the same file.

mod addon;
mod app_update;
mod commands;
mod links;
mod settings;
mod state;
mod token;
mod workers;

use mythics_logger_core::queue::Queue;
use mythics_logger_core::throttle::Throttle;
use state::AppState;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

/// Passed by the "start with Windows" entry: start in the tray, not a window.
const MINIMISED_ARG: &str = "--minimised";

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

const TRAY_ID: &str = "main";

/// The tray menu's live logging item, kept to change its words.
struct TrayLive(MenuItem<tauri::Wry>);

fn tray_live_label(on: bool) -> &'static str {
    if on {
        "Turn live logging off"
    } else {
        "Turn live logging on"
    }
}

fn tray_tooltip(on: bool) -> &'static str {
    if on {
        "mythics.gg Logger · Live logging on"
    } else {
        "mythics.gg Logger · Live logging off"
    }
}

/// Makes the tray say whether live logging is on, after it's switched from
/// anywhere.
pub fn update_tray(app: &AppHandle) {
    let on = app
        .state::<Arc<AppState>>()
        .settings
        .lock()
        .expect("settings")
        .live_logging;
    if let Some(item) = app.try_state::<TrayLive>() {
        let _ = item.0.set_text(tray_live_label(on));
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tray_tooltip(on)));
    }
}

pub fn run() {
    tauri::Builder::default()
        // First, so a second launch only brings this one forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                // Library chatter can include URLs; keep it to warnings.
                .level_for("reqwest", log::LevelFilter::Warn)
                .level_for("hyper", log::LevelFilter::Warn)
                .level_for("hyper_util", log::LevelFilter::Warn)
                .level_for("rustls", log::LevelFilter::Warn)
                .max_file_size(2_000_000)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![MINIMISED_ARG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            std::fs::create_dir_all(&data_dir)?;
            let settings = settings::Settings::load(&config_dir.join("settings.json"));
            let throttle = Arc::new(Throttle::new(settings.upload_limit_kbps as u64 * 1000));
            let state = Arc::new(AppState {
                queue: Arc::new(Mutex::new(Queue::load(&data_dir))),
                settings: Mutex::new(settings),
                live: Mutex::new(state::LiveStatus {
                    status: "searching",
                    ..Default::default()
                }),
                backlog: Mutex::new(Default::default()),
                throttle,
                token: Mutex::new(token::load()),
                progress: Mutex::new(Default::default()),
                server_shas: Mutex::new(Default::default()),
                server_uploads: Mutex::new(Default::default()),
                signing_in: AtomicBool::new(false),
                cancel_sign_in: AtomicBool::new(false),
                wake: tokio::sync::Notify::new(),
                signed_out_notice: AtomicBool::new(false),
                archive: Mutex::new(Default::default()),
                skips: Mutex::new(mythics_logger_core::archive::Skips::load(
                    &data_dir.join("skipped-logs.json"),
                )),
                addon: Mutex::new(Default::default()),
                app_update: Mutex::new(Default::default()),
                app_update_wake: tokio::sync::Notify::new(),
                addon_wake: tokio::sync::Notify::new(),
                config_dir,
                data_dir,
            });
            app.manage(state.clone());
            log::info!(
                "mythics.gg Logger {} started",
                mythics_logger_core::CLIENT_VERSION
            );

            let live_on = state.settings.lock().expect("settings").live_logging;
            let open =
                MenuItem::with_id(app, "open", "Open mythics.gg Logger", true, None::<&str>)?;
            let live =
                MenuItem::with_id(app, "live", tray_live_label(live_on), true, None::<&str>)?;
            let my_logs = MenuItem::with_id(app, "my_logs", "Open My logs", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &live, &my_logs, &quit])?;
            app.manage(TrayLive(live));
            let mut tray = TrayIconBuilder::with_id(TRAY_ID)
                .tooltip(tray_tooltip(live_on))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "live" => {
                        let state = app.state::<Arc<AppState>>();
                        commands::toggle_live(app, &state);
                    }
                    "my_logs" => {
                        let state = app.state::<Arc<AppState>>();
                        if commands::open_page(app, &state, links::MY_LOGS).is_err() {
                            log::warn!("couldn't open My logs in the browser");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = e
                    {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let minimised = std::env::args().any(|a| a == MINIMISED_ARG);
            if !minimised {
                show_main(app.handle());
            }

            let (h, s) = (app.handle().clone(), state.clone());
            std::thread::Builder::new()
                .name("tail".into())
                .spawn(move || workers::tail_forever(h, s))?;
            let (h, s) = (app.handle().clone(), state.clone());
            tauri::async_runtime::spawn(workers::upload_forever(h, s));
            let (h, s) = (app.handle().clone(), state.clone());
            tauri::async_runtime::spawn(workers::poll_uploads_forever(h, s));
            let (h, s) = (app.handle().clone(), state.clone());
            tauri::async_runtime::spawn(addon::addon_forever(h, s));
            let (h, s) = (app.handle().clone(), state.clone());
            tauri::async_runtime::spawn(app_update::app_update_forever(h, s));
            let (h, s) = (app.handle().clone(), state);
            std::thread::Builder::new()
                .name("archive".into())
                .spawn(move || workers::archive_forever(h, s))?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app in the tray, still logging.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::log_in,
            commands::cancel_log_in,
            commands::log_out,
            commands::save_settings,
            commands::choose_logs_folder,
            commands::find_logs_folder,
            commands::backlog_scan,
            commands::backlog_choose_files,
            commands::backlog_cancel,
            commands::backlog_upload,
            commands::backlog_pause,
            commands::archive_log,
            commands::backlog_skip,
            commands::open_archive_folder,
            commands::history,
            commands::recent_uploads,
            commands::set_upload_visibility,
            commands::delete_upload,
            commands::open_site,
            commands::open_log,
            addon::addon_check,
            addon::addon_install,
            app_update::app_update_check,
            app_update::app_update_later,
            app_update::app_update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the mythics.gg Logger");
}
