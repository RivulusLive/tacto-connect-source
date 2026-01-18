// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

use crate::UI_CONTEXT;
use crate::keyboard::{COLUMNS, ROWS};

use std::sync::Arc;

use eframe::egui;
use futures::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::json;
use tokio::spawn;
use tokio::sync::{Mutex, broadcast};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

pub fn json_message(msg: serde_json::Value) -> Message {
	Message::Text(msg.to_string().into())
}

pub async fn init_openaction(
	mut args: Vec<String>,
) -> (
	Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Message>>>,
	broadcast::Receiver<Message>,
) {
	struct CliArgs {
		_command: String,
		port: String,
		uuid: String,
		event: String,
	}
	let args = CliArgs {
		_command: args.remove(0),
		port: args.remove(1),
		uuid: args.remove(2),
		event: args.remove(3),
	};

	let (ws, _) = connect_async(format!("ws://localhost:{}", args.port))
		.await
		.unwrap();

	let (mut tx, mut rx) = ws.split();
	tx.send(json_message(json!({
		"event": args.event,
		"uuid": args.uuid,
	})))
	.await
	.unwrap();
	let to_openaction = Arc::new(Mutex::new(tx));
	let (sender, receiver) = broadcast::channel(16);

	spawn(async move {
		while let Some(message) = rx.next().await {
			let Ok(message) = message else {
				continue;
			};
			let Message::Text(text) = &message else {
				continue;
			};
			let Ok(data): Result<serde_json::Value, _> = serde_json::from_str(text) else {
				continue;
			};

			if data["event"] == "setImage" && data["device"] == "rl-keyboard" {
				let Some(context) = &*UI_CONTEXT.read().unwrap() else {
					continue;
				};

				if data["position"].is_i64() {
					let position = data["position"].as_i64().unwrap() as usize;
					let id = egui::Id::new(position as u8);

					if let Some(uri) = data["image"].as_str() {
						use base64::Engine;
						if let Ok(data) = base64::engine::general_purpose::STANDARD
							.decode(&uri["data:image/jpeg;base64,".len()..])
							&& let Some(image) = image::ImageReader::new(std::io::Cursor::new(data))
								.with_guessed_format()
								.ok()
								.and_then(|x| x.decode().ok())
						{
							let color_image = egui::ColorImage::from_rgb(
								[image.width() as usize, image.height() as usize],
								&image.to_rgb8(),
							);
							let texture =
								context.load_texture(uri, color_image, Default::default());
							context.data_mut(|d| d.insert_temp(id, texture));
						}
					} else {
						context.data_mut(|d| d.remove::<egui::TextureHandle>(id));
					}
				} else {
					for position in 0..(ROWS as usize * COLUMNS as usize) {
						let id = egui::Id::new(position as u8);
						context.data_mut(|d| d.remove::<egui::TextureHandle>(id));
					}
				}

				context.request_repaint();
			} else if data["event"] == "showSettingsInterface" {
				#[cfg(target_os = "linux")]
				crate::WINDOW_REOPEN_SIGNAL.notify_one();

				#[cfg(target_os = "windows")]
				if !crate::get_settings().keyboard_enabled {
					crate::WINDOW_REOPEN_SIGNAL.notify_one();
					continue;
				}

				#[cfg(not(target_os = "linux"))]
				{
					let context = UI_CONTEXT.read().unwrap();
					if let Some(context) = &*context {
						use eframe::egui;
						crate::ui::set_window_visibility(context, true);
						context.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
							egui::UserAttentionType::Critical,
						));
						context.send_viewport_cmd(egui::ViewportCommand::Focus);
					}
				}
			}

			let _ = sender.send(message);
		}

		std::process::exit(0);
	});

	(to_openaction, receiver)
}
