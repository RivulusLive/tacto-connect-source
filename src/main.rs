// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod keyboard;
mod mobile;
mod openaction;
mod ui;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Arc, LazyLock};

use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use tokio::spawn;
use tokio::sync::broadcast;

static UI_CONTEXT: LazyLock<std::sync::RwLock<Option<eframe::egui::Context>>> =
	LazyLock::new(|| std::sync::RwLock::new(None));

#[cfg(any(target_os = "linux", target_os = "windows"))]
use tokio::sync::Notify;
#[cfg(any(target_os = "linux", target_os = "windows"))]
static WINDOW_REOPEN_SIGNAL: LazyLock<Arc<Notify>> = LazyLock::new(|| Arc::new(Notify::new()));

const SETTINGS_FILE: &str = "../../settings/us.rivul.tacto.sdPlugin.json";

const ORIGIN: &str = "https://tacto.live";

#[derive(Clone, Serialize, Deserialize)]
struct SessionConfig {
	email: String,
	user_id: String,
	session_uuid: String,
	#[serde(default)]
	is_guest: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Settings {
	session: Option<SessionConfig>,
	hot_key_mappings: HashMap<HotKey, HashSet<u8>>,
	#[serde(default)]
	keyboard_enabled: bool,
}

fn get_settings() -> Settings {
	serde_json::from_slice(&fs::read(SETTINGS_FILE).unwrap_or_default()).unwrap_or_default()
}

fn set_settings(value: &Settings) -> std::io::Result<()> {
	fs::write(SETTINGS_FILE, serde_json::to_string_pretty(&value)?)
}

#[tokio::main]
async fn main() -> eframe::Result {
	if let Err(error) = simplelog::TermLogger::init(
		simplelog::LevelFilter::Debug,
		simplelog::ConfigBuilder::default()
			.add_filter_ignore_str("sctk")
			.add_filter_ignore_str("winit")
			.add_filter_ignore_str("tracing::span")
			.add_filter_ignore_str("eframe::native")
			.add_filter_ignore_str("reqwest::connect")
			.add_filter_ignore_str("hyper_util::client")
			.add_filter_ignore_str("webrtc_sctp")
			.build(),
		simplelog::TerminalMode::Stdout,
		simplelog::ColorChoice::Never,
	) {
		eprintln!("Logger initialization failed: {}", error);
	}

	let _ = fs::create_dir_all(std::path::Path::new(SETTINGS_FILE).parent().unwrap());

	let args: Vec<String> = std::env::args().collect();
	let (to_openaction, from_openaction) = openaction::init_openaction(args).await;

	let (etx, erx) = broadcast::channel::<Option<Vec<String>>>(16);

	spawn(mobile::init_mobile(
		to_openaction.clone(),
		from_openaction.resubscribe(),
		erx.resubscribe(),
	));

	let hot_key_manager = Arc::new(GlobalHotKeyManager::new().unwrap());
	for hot_key in get_settings().hot_key_mappings.keys() {
		if hot_key_manager.register(*hot_key).is_ok() {
			log::debug!("[keyboard] Hot key {hot_key} registered");
		}
	}

	spawn(keyboard::init_keyboard(to_openaction, erx));

	if let Some(session) = get_settings().session
		&& session.is_guest
	{
		let _ = fs::remove_file(SETTINGS_FILE);
	}

	let initial_result = get_settings()
		.session
		.as_ref()
		.map(|session| auth::get_entitlements(&session.user_id, &session.session_uuid));

	let entitlements = match initial_result {
		Some(Ok(ents)) => {
			let _ = etx.send(Some(ents.clone()));
			Some(ents)
		}
		Some(Err(auth::AuthError::Network(_))) => {
			let _ = etx.send(None);

			let etx = etx.clone();
			spawn(async move {
				loop {
					let Some(session) = get_settings().session else {
						break;
					};
					match auth::get_entitlements(&session.user_id, &session.session_uuid) {
						Ok(ents) => {
							let _ = etx.send(Some(ents));
							if let Some(context) = UI_CONTEXT.read().unwrap().as_ref() {
								context.request_repaint();
							}
							break;
						}
						Err(auth::AuthError::Network(_)) => {}
						Err(auth::AuthError::Auth(_)) | Err(auth::AuthError::IO(_)) => break,
					}
					tokio::time::sleep(std::time::Duration::from_secs(5)).await;
				}
			});

			None
		}
		Some(Err(auth::AuthError::Auth(_))) | Some(Err(auth::AuthError::IO(_))) | None => {
			let _ = etx.send(None);
			None
		}
	};

	#[cfg(target_os = "linux")]
	if fs::exists(SETTINGS_FILE).unwrap_or(false) && entitlements.is_some() {
		WINDOW_REOPEN_SIGNAL.notified().await;
	}

	#[cfg(not(target_os = "linux"))]
	let mut start_hidden = true;
	#[cfg(not(target_os = "linux"))]
	if !fs::exists(SETTINGS_FILE).unwrap_or(false) || entitlements.is_none() {
		start_hidden = false;
	}

	#[cfg(target_os = "windows")]
	if start_hidden && !get_settings().keyboard_enabled {
		WINDOW_REOPEN_SIGNAL.notified().await;
		start_hidden = false;
	}

	#[allow(clippy::never_loop)]
	loop {
		let result = eframe::run_native(
			"Tacto",
			eframe::NativeOptions {
				viewport: eframe::egui::ViewportBuilder::default().with_icon(
					eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
						.unwrap(),
				),
				..Default::default()
			},
			Box::new(|cc| {
				*crate::UI_CONTEXT.write().unwrap() = Some(cc.egui_ctx.clone());
				#[cfg(not(target_os = "linux"))]
				if start_hidden {
					ui::set_window_visibility(&cc.egui_ctx, false);
				}

				Ok(Box::new(ui::App::new(
					entitlements.clone(),
					etx.clone(),
					hot_key_manager.clone(),
				)))
			}),
		);

		#[cfg(target_os = "linux")]
		{
			let _ = result;
			WINDOW_REOPEN_SIGNAL.notified().await;
		}

		#[cfg(target_os = "windows")]
		if !get_settings().keyboard_enabled {
			WINDOW_REOPEN_SIGNAL.notified().await;
			continue;
		}

		#[cfg(not(target_os = "linux"))]
		return result;
	}
}
