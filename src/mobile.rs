// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

use crate::get_settings;
use crate::openaction::json_message;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic;
use std::time::Duration;

use futures::executor::block_on;
use futures::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::spawn;
use tokio::sync::{Mutex, Notify, broadcast};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use webrtc::{
	api::APIBuilder, api::setting_engine::SettingEngine,
	data_channel::data_channel_message::DataChannelMessage,
	data_channel::data_channel_state::RTCDataChannelState, ice::network_type::NetworkType,
	ice_transport::ice_candidate::RTCIceCandidateInit, ice_transport::ice_server::RTCIceServer,
	peer_connection::configuration::RTCConfiguration,
	peer_connection::peer_connection_state::RTCPeerConnectionState,
	peer_connection::sdp::session_description::RTCSessionDescription,
};

async fn list_signalling_servers() -> Vec<String> {
	match async {
		let response = reqwest::get(format!("{}/signalling_servers.json", crate::ORIGIN)).await?;
		let bytes = response.bytes().await?;
		Ok::<Vec<String>, anyhow::Error>(serde_json::from_slice(&bytes)?)
	}
	.await
	{
		Ok(servers) => servers,
		Err(error) => {
			log::warn!("[signalling] Failed to fetch signalling servers: {}", error);
			vec![
				"wss://signalling.tacto.live".to_owned(),
				"wss://signalling-backup.tacto.live".to_owned(),
			]
		}
	}
}

async fn list_ice_servers(user_id: &str) -> Vec<RTCIceServer> {
	let client = reqwest::Client::new();
	match async {
		let response = client
			.get(format!("{}/ice_servers.json", crate::ORIGIN))
			.header(reqwest::header::COOKIE, format!("userId={}", user_id))
			.send()
			.await?;
		let bytes = response.bytes().await?;
		Ok::<Vec<RTCIceServer>, anyhow::Error>(serde_json::from_slice(&bytes)?)
	}
	.await
	{
		Ok(servers) => servers,
		Err(error) => {
			log::warn!("[peer] Failed to fetch ICE servers: {}", error);
			vec![
				RTCIceServer {
					urls: vec!["stun:stun.l.google.com:19302".to_owned()],
					..Default::default()
				},
				RTCIceServer {
					urls: vec![
						"stun:stun.cloudflare.com:3478".to_owned(),
						"stun:stun.cloudflare.com:53".to_owned(),
					],
					..Default::default()
				},
			]
		}
	}
}

pub static CONNECTED: atomic::AtomicBool = atomic::AtomicBool::new(false);
fn set_connected(connected: bool) {
	CONNECTED.store(connected, atomic::Ordering::Relaxed);
	if let Some(context) = crate::UI_CONTEXT.read().unwrap().as_ref() {
		context.request_repaint();
	}
}

async fn connect(
	user_id: &str,
	session_uuid: &str,
	entitlements: Vec<String>,
	to_openaction: Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>,
	from_openaction: broadcast::Receiver<Message>,
	abort: Arc<Notify>,
) -> anyhow::Result<()> {
	log::debug!("[peer] Creating connection");
	let mut settings = SettingEngine::default();
	settings.set_network_types(vec![NetworkType::Udp4, NetworkType::Tcp4]);
	let api = APIBuilder::new().with_setting_engine(settings).build();
	let pc = api
		.new_peer_connection(RTCConfiguration {
			ice_servers: list_ice_servers(user_id).await,
			..Default::default()
		})
		.await?;

	let device_id = if user_id == "guest" {
		"rl-mobile-guest".to_owned()
	} else {
		let id = format!("rl-mobile-{}", user_id.split_once('-').unwrap().0);

		let profiles_path = Path::new("../../profiles").join(&id);
		let guest_profiles_path = Path::new("../../profiles").join("rl-mobile-guest");
		if !profiles_path.exists() && guest_profiles_path.exists() {
			pub fn copy_dir(
				src: impl AsRef<Path>,
				dst: impl AsRef<Path>,
			) -> Result<(), std::io::Error> {
				use std::fs;
				fs::create_dir_all(&dst)?;
				for entry in fs::read_dir(src)?.flatten() {
					if entry.file_type()?.is_dir() {
						copy_dir(entry.path(), dst.as_ref().join(entry.file_name()))?;
					} else {
						fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
					}
				}
				Ok(())
			}
			let _ = copy_dir(guest_profiles_path, profiles_path);
			let _ = copy_dir(
				Path::new("../../images").join("rl-mobile-guest"),
				Path::new("../../images").join(&id),
			);
		}

		id
	};
	pc.on_data_channel(Box::new(move |dc| {
		dc.on_open(Box::new(move || {
			log::debug!("[peer] Data channel opened");
			set_connected(true);
			Box::pin(async {})
		}));

		let to_openaction_1 = to_openaction.clone();
		let device_id_1 = device_id.clone();
		let entitlements = entitlements.clone();
		dc.on_message(Box::new(move |msg: DataChannelMessage| {
			let data = String::from_utf8(msg.data.to_vec()).unwrap();
			log::debug!("[peer] Data channel message received: {}", data);

			let (action, payload) = data.split_once(",").unwrap_or((&data, ""));
			block_on(async {
				if action == "key_down" || action == "key_up" {
					let Ok(position) = payload.parse::<u8>() else {
						return;
					};

					let _ = to_openaction_1
						.lock()
						.await
						.send(json_message(json!({
							"event": if action == "key_up" { "keyUp" } else { "keyDown" },
							"payload": {
								"device": device_id_1,
								"position": position,
							},
						})))
						.await;
				} else if action == "resize" {
					let (Some(rows), Some(columns)) = (
						data.split(',').nth(1).and_then(|p| p.parse::<u8>().ok()),
						data.split(',').nth(2).and_then(|p| p.parse::<u8>().ok()),
					) else {
						return;
					};

					if !entitlements.contains(&"mobileResizeDevice".to_owned())
						&& (rows * columns) > 6
					{
						return;
					}

					let _ = to_openaction_1
						.lock()
						.await
						.send(json_message(json!({
							"event": "deregisterDevice",
							"payload": device_id_1,
						})))
						.await;
					let _ = to_openaction_1
						.lock()
						.await
						.send(json_message(json!({
							"event": "registerDevice",
							"payload": {
								"id": device_id_1,
								"name": "Tacto Mobile",
								"rows": rows,
								"columns": columns,
								"encoders": 0,
								"type": 1,
							},
						})))
						.await;

					sleep(Duration::from_millis(100)).await;
					let _ = to_openaction_1
						.lock()
						.await
						.send(json_message(json!({
							"event": "rerenderImages",
							"payload": device_id_1,
						})))
						.await;
				}
			});

			Box::pin(async {})
		}));

		let to_openaction_2 = to_openaction.clone();
		let device_id_2 = device_id.clone();
		dc.on_close(Box::new(move || {
			log::debug!("[peer] Data channel closed");
			set_connected(false);
			block_on(async {
				let _ = to_openaction_2
					.lock()
					.await
					.send(json_message(json!({
						"event": "deregisterDevice",
						"payload": device_id_2,
					})))
					.await;
			});
			Box::pin(async {})
		}));

		dc.on_error(Box::new(move |error| {
			log::error!("[peer] Data channel error: {}", error);
			Box::pin(async {})
		}));

		let mut from_openaction = from_openaction.resubscribe();
		let device_id_3 = device_id.clone();
		spawn(async move {
			loop {
				let recv = from_openaction.recv().await;
				let message = match recv {
					Ok(msg) => msg,
					Err(broadcast::error::RecvError::Closed) => break,
					Err(broadcast::error::RecvError::Lagged(_)) => continue,
				};
				if dc.ready_state() != RTCDataChannelState::Open {
					break;
				}
				let Ok(text) = message.into_text() else {
					continue;
				};
				let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
					continue;
				};
				if value["event"] != "setImage" || value["device"] != device_id_3 {
					continue;
				}
				let _ = if value["image"].is_string() {
					dc.send(
						&(value["position"].as_number().unwrap().to_string()
							+ "," + value["image"].as_str().unwrap())
						.into(),
					)
					.await
				} else if value["position"].is_number() {
					dc.send(&(value["position"].as_number().unwrap().to_string() + ",").into())
						.await
				} else {
					dc.send(&",".into()).await
				};
			}
		});

		Box::pin(async {})
	}));

	let mut ws_stream = Err(anyhow::anyhow!("No signalling servers available"));

	for server in &list_signalling_servers().await {
		log::debug!("[signalling] Connecting to server: {}", server);

		match async {
			let (mut stream, _) = connect_async(server).await?;

			stream
				.send(json_message(
					json!({ "event": "registration", "type": "plugin", "userId": user_id, "sessionUuid": session_uuid })
				))
				.await?;

			log::debug!("[signalling] Connected to server");
			Ok::<_, tokio_tungstenite::tungstenite::Error>(stream)
		}.await {
			Ok(stream) => {
				ws_stream = Ok(stream);
				break;
			}
			Err(error) => log::warn!("[signalling] Failed to connect to {}: {}", server, error)
		}
	}

	let (mut to_signalling, mut from_signalling) = ws_stream?.split();

	to_signalling
		.send(json_message(json!({ "event": "version", "data": 1 })))
		.await?;

	let to_signalling_1 = Arc::new(Mutex::new(to_signalling));
	let to_signalling_2 = to_signalling_1.clone();

	pc.on_ice_candidate(Box::new(move |cand| {
		if let Some(candidate) = cand {
			log::debug!("[signalling] Sending ICE candidate: {}", candidate);
			block_on(async {
				let _ = to_signalling_1
					.lock()
					.await
					.send(json_message(
						json!({ "event": "candidate", "data": candidate.to_json().unwrap() }),
					))
					.await;
			});
		}
		Box::pin(async {})
	}));

	while let Some(Ok(msg)) = from_signalling.next().await {
		let Ok(text) = msg.into_text() else {
			continue;
		};
		let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
			continue;
		};

		log::debug!(
			"[signalling] Received {} message: {}",
			value["event"],
			value["data"]
		);
		match value["event"].as_str().unwrap_or("") {
			"offer" => {
				let offer = serde_json::from_value::<RTCSessionDescription>(value["data"].clone())?;
				pc.set_remote_description(offer).await?;

				let answer = pc.create_answer(None).await?;
				pc.set_local_description(answer.clone()).await?;

				to_signalling_2
					.lock()
					.await
					.send(json_message(json!({ "event": "answer", "data": answer })))
					.await?;
			}
			"candidate" => {
				let Ok(cand) = serde_json::from_value::<RTCIceCandidateInit>(value["data"].clone())
				else {
					log::warn!("[signalling] Failed to parse ICE candidate");
					continue;
				};
				let _ = pc.add_ice_candidate(cand).await;
			}
			_ => {}
		}
		if pc.connection_state() == RTCPeerConnectionState::Connected {
			break;
		}
	}
	log::debug!("[signalling] Disconnected from server");

	let notify_1 = Arc::new(Notify::new());
	let notify_2 = notify_1.clone();
	if !matches!(
		pc.connection_state(),
		RTCPeerConnectionState::Connecting | RTCPeerConnectionState::Connected,
	) {
		notify_1.notify_one();
	}
	pc.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
		log::debug!("[peer] Connection state changed: {}", state);
		if state != RTCPeerConnectionState::Connected {
			notify_1.notify_one();
		}
		Box::pin(async {})
	}));
	tokio::select! {
		_ = notify_2.notified() => {},
		_ = abort.notified() => {
			log::debug!("[peer] Abort signal received");
		},
	}
	pc.close().await?;
	log::debug!("[peer] Connection closed");

	Ok(())
}

pub async fn init_mobile(
	to_openaction: Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>,
	from_openaction: broadcast::Receiver<Message>,
	mut erx: broadcast::Receiver<Option<Vec<String>>>,
) {
	let mut handle: Option<JoinHandle<()>> = None;
	let abort = Arc::new(Notify::new());

	while let Ok(entitlements) = erx.recv().await {
		if let Some(entitlements) = entitlements {
			let settings = get_settings();
			let Some(session_config) = settings.session else {
				continue;
			};
			if let Some(handle) = &handle {
				handle.abort();
			}
			let (to_openaction, from_openaction) =
				(to_openaction.clone(), from_openaction.resubscribe());
			let abort = abort.clone();
			handle = Some(spawn(async move {
				loop {
					let (to, from) = (to_openaction.clone(), from_openaction.resubscribe());
					if let Err(error) = connect(
						&session_config.user_id,
						&session_config.session_uuid,
						entitlements.clone(),
						to,
						from,
						abort.clone(),
					)
					.await
					{
						log::error!("[peer] Failed to establish connection: {}", error);
					}

					// If guest session disconnects, clear it and stop reconnecting
					if session_config.is_guest {
						let mut settings = get_settings();
						settings.session = None;
						let _ = crate::set_settings(&settings);
						break;
					}
				}
			}));
		} else if let Some(handle) = &handle
			&& !handle.is_finished()
		{
			abort.notify_one();
		}
	}
}
