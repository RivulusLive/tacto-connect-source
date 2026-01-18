// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

use crate::openaction::json_message;
use crate::{get_settings, set_settings};

use std::str::FromStr;
use std::sync::Arc;

use futures::{SinkExt, stream::SplitSink};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::spawn;
use tokio::sync::{Mutex, broadcast};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

#[rustfmt::skip]
pub const KEYS: [&str; 121] = ["ALT", "CONTROL", "SUPER", "SHIFT", "BACKQUOTE", "BACKSLASH", "BRACKETLEFT", "BRACKETRIGHT", "PAUSEBREAK", "COMMA", "DIGIT0", "DIGIT1", "DIGIT2", "DIGIT3", "DIGIT4", "DIGIT5", "DIGIT6", "DIGIT7", "DIGIT8", "DIGIT9", "EQUAL", "KEYA", "KEYB", "KEYC", "KEYD", "KEYE", "KEYF", "KEYG", "KEYH", "KEYI", "KEYJ", "KEYK", "KEYL", "KEYM", "KEYN", "KEYO", "KEYP", "KEYQ", "KEYR", "KEYS", "KEYT", "KEYU", "KEYV", "KEYW", "KEYX", "KEYY", "KEYZ", "MINUS", "PERIOD", "QUOTE", "SEMICOLON", "SLASH", "BACKSPACE", "CAPSLOCK", "ENTER", "SPACE", "TAB", "DELETE", "END", "HOME", "INSERT", "PAGEDOWN", "PAGEUP", "PRINTSCREEN", "SCROLLLOCK", "ARROWDOWN", "ARROWLEFT", "ARROWRIGHT", "ARROWUP", "NUMLOCK", "NUMPAD0", "NUMPAD1", "NUMPAD2", "NUMPAD3", "NUMPAD4", "NUMPAD5", "NUMPAD6", "NUMPAD7", "NUMPAD8", "NUMPAD9", "NUMPADADD", "NUMPADDECIMAL", "NUMPADDIVIDE", "NUMPADENTER", "NUMPADEQUAL", "NUMPADMULTIPLY", "NUMPADSUBTRACT", "ESCAPE", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24", "AUDIOVOLUMEDOWN", "AUDIOVOLUMEUP", "AUDIOVOLUMEMUTE", "MEDIAPLAY", "MEDIAPAUSE", "MEDIAPLAYPAUSE", "MEDIASTOP", "MEDIATRACKNEXT", "MEDIATRACKPREV"];

pub const ROWS: u8 = 6;
pub const COLUMNS: u8 = 8;

pub async fn add_mapping(
	manager: &GlobalHotKeyManager,
	hot_key: String,
	position: u8,
) -> Result<(), anyhow::Error> {
	log::debug!("[keyboard] Adding mapping {hot_key} to {position}");
	let hot_key = HotKey::from_str(&hot_key)?;
	manager.register(hot_key)?;
	log::debug!("[keyboard] Hot key {hot_key} registered");
	let mut settings = get_settings();
	settings
		.hot_key_mappings
		.entry(hot_key)
		.or_default()
		.insert(position);
	set_settings(&settings)?;
	log::debug!("[keyboard] Mapping {hot_key} added to {position}");
	Ok(())
}

pub async fn remove_mapping(
	manager: &GlobalHotKeyManager,
	hot_key: HotKey,
	position: u8,
) -> Result<(), anyhow::Error> {
	log::debug!("[keyboard] Removing mapping {hot_key} from {position}");
	let mut settings = get_settings();
	let mappings = &mut settings.hot_key_mappings;
	mappings.get_mut(&hot_key).unwrap().remove(&position);
	if mappings.get(&hot_key).unwrap().is_empty() {
		manager.unregister(hot_key)?;
		mappings.remove(&hot_key);
		log::debug!("[keyboard] Hot key {hot_key} unregistered");
	}
	set_settings(&settings)?;
	log::debug!("[keyboard] Mapping {hot_key} removed from {position}");
	Ok(())
}

pub async fn init_keyboard(
	to_openaction: Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>,
	mut erx: broadcast::Receiver<Option<Vec<String>>>,
) {
	log::debug!("[keyboard] Initialising keyboard");
	let to_openaction_2 = to_openaction.clone();

	let entitlements = std::sync::Arc::new(tokio::sync::RwLock::new(None));
	let entitlements_2 = entitlements.clone();

	spawn(async move {
		while let Ok(e) = erx.recv().await {
			if e.is_some() && get_settings().keyboard_enabled {
				log::debug!("[keyboard] Registering device");
				let _ = to_openaction
					.lock()
					.await
					.send(json_message(json!({
						"event": "registerDevice",
						"payload": {
							"id": "rl-keyboard",
							"name": "Tacto Keyboard",
							"rows": ROWS,
							"columns": COLUMNS,
							"encoders": 0,
							"type": 0
						}
					})))
					.await;
			} else {
				log::debug!("[keyboard] Deregistering device");
				let _ = to_openaction
					.lock()
					.await
					.send(json_message(json!({
						"event": "deregisterDevice",
						"payload": "rl-keyboard"
					})))
					.await;
			}

			*entitlements.write().await = e.clone();
		}
	});

	let entitlement = "keyboardUnlimited".to_owned();
	spawn(async move {
		loop {
			if let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
				log::debug!("[keyboard] Hot key {} became {:?}", event.id, event.state);
				let direction = match event.state {
					HotKeyState::Pressed => "keyDown",
					HotKeyState::Released => "keyUp",
				};

				if let Some((_, positions)) = get_settings()
					.hot_key_mappings
					.iter()
					.find(|x| x.0.id == event.id)
					&& let Some(entitlements) = &*entitlements_2.read().await
				{
					for position in positions {
						if !entitlements.contains(&entitlement) && *position >= 6 {
							continue;
						}
						let _ = to_openaction_2
							.lock()
							.await
							.send(json_message(json!({
								"event": direction,
								"payload": {
									"device": "rl-keyboard",
									"position": position
								}
							})))
							.await;
					}
				}
			}
		}
	});
}
