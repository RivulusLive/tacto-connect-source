// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

mod keyboard;
mod qr;

use crate::auth::{
	QrPollResult, complete_guest_login, complete_qr_login, get_entitlements, log_in, log_out,
};
use crate::get_settings;
use crate::keyboard::{COLUMNS, ROWS};

use eframe::egui::{
	self, Color32, ComboBox, Context, Layout, MenuBar, RichText, TextEdit, Ui, ViewportCommand,
	vec2,
};

use global_hotkey::GlobalHotKeyManager;
use tokio::sync::broadcast::{Receiver, Sender};

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
	match std::env::var("XDG_SESSION_TYPE") {
		Ok(val) if val.as_str() == "wayland" => true,
		Ok(val) if val.as_str() == "x11" => false,
		_ if std::env::var("WAYLAND_DISPLAY").is_ok() => true,
		_ => false,
	}
}

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
static VISIBILITY_CHANGED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static IS_VISIBLE: AtomicBool = AtomicBool::new(false);

pub fn set_window_visibility(ctx: &Context, visible: bool) {
	#[cfg(target_os = "linux")]
	if visible {
		crate::WINDOW_REOPEN_SIGNAL.notify_one();
		let _ = ctx;
	}
	#[cfg(target_os = "windows")]
	ctx.send_viewport_cmd(ViewportCommand::Minimized(!visible));
	#[cfg(not(any(target_os = "linux", target_os = "windows")))]
	ctx.send_viewport_cmd(ViewportCommand::Visible(visible));
	#[cfg(target_os = "macos")]
	{
		VISIBILITY_CHANGED.store(true, Ordering::Relaxed);
		IS_VISIBLE.store(visible, Ordering::Relaxed);
	}
}

struct WindowState {
	last_min_inner_size: Option<egui::Vec2>,
	last_max_inner_size: Option<egui::Vec2>,
}

impl WindowState {
	fn new() -> Self {
		Self {
			last_min_inner_size: None,
			last_max_inner_size: None,
		}
	}

	fn set_min_size(&mut self, ctx: &Context, size: egui::Vec2) {
		if self.last_min_inner_size != Some(size) {
			ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(size));
			self.last_min_inner_size = Some(size);
		}
	}

	fn set_max_size(&mut self, ctx: &Context, size: egui::Vec2) {
		if self.last_max_inner_size != Some(size) {
			ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(size));
			self.last_max_inner_size = Some(size);
		}
	}
}

struct AuthState {
	email: String,
	password: String,
	error: Option<String>,
	entitlements: Option<Vec<String>>,
	etx: Sender<Option<Vec<String>>>,
	erx: Receiver<Option<Vec<String>>>,
	last_content_height: f32,
	show_email_login: bool,
	qr: qr::QrState,
	pending_qr_result: Option<QrPollResult>,
}

struct KeyboardState {
	enabled: bool,
	manager: std::sync::Arc<GlobalHotKeyManager>,
	selected_position: Option<u8>,
	selected_key: &'static str,
	building_hot_key: Vec<&'static str>,
	errors: Vec<anyhow::Error>,
}

pub struct App {
	selected_device: &'static str,
	auth: AuthState,
	keyboard: KeyboardState,
	window_state: WindowState,
}

impl App {
	pub fn new(
		entitlements: Option<Vec<String>>,
		etx: Sender<Option<Vec<String>>>,
		hot_key_manager: std::sync::Arc<GlobalHotKeyManager>,
	) -> Self {
		let erx = etx.subscribe();
		Self {
			selected_device: "Mobile",
			auth: AuthState {
				email: String::new(),
				password: String::new(),
				error: None,
				entitlements,
				etx,
				erx,
				last_content_height: 0.0,
				show_email_login: false,
				qr: qr::QrState::new(),
				pending_qr_result: None,
			},
			keyboard: KeyboardState {
				enabled: get_settings().keyboard_enabled,
				manager: hot_key_manager,
				selected_position: None,
				selected_key: "",
				building_hot_key: Vec::new(),
				errors: Vec::new(),
			},
			window_state: WindowState::new(),
		}
	}
}

impl eframe::App for App {
	fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
		if let Ok(entitlements) = self.auth.erx.try_recv() {
			self.auth.entitlements = entitlements;
			if self.auth.entitlements.is_some() {
				self.auth.error = None;
			}
		}

		if ctx.input(|i| i.viewport().close_requested()) {
			#[cfg(target_os = "linux")]
			return;

			#[cfg(target_os = "windows")]
			if !get_settings().keyboard_enabled {
				return;
			}

			#[cfg(not(target_os = "linux"))]
			{
				ctx.send_viewport_cmd(ViewportCommand::CancelClose);
				set_window_visibility(ctx, false);
			}
		}

		#[cfg(target_os = "macos")]
		if VISIBILITY_CHANGED.swap(false, Ordering::Relaxed) {
			objc2_app_kit::NSApp(objc2::MainThreadMarker::new().unwrap()).setActivationPolicy(
				if !IS_VISIBLE.load(Ordering::Relaxed) {
					objc2_app_kit::NSApplicationActivationPolicy::Accessory
				} else {
					objc2_app_kit::NSApplicationActivationPolicy::Regular
				},
			);
		}

		let settings = get_settings();

		if let Some(session) = &settings.session {
			if session.is_guest {
				// Guest sessions don't validate entitlements with server
				if self.auth.entitlements.is_none() {
					self.auth.entitlements = Some(vec![]);
					let _ = self.auth.etx.send(self.auth.entitlements.clone());
				}
			} else if self.auth.entitlements.is_none() && self.auth.error.is_none() {
				match get_entitlements(&session.user_id, &session.session_uuid) {
					Ok(entitlements) => {
						self.auth.entitlements = Some(entitlements);
						let _ = self.auth.etx.send(self.auth.entitlements.clone());
					}
					Err(error) => {
						self.auth.error = Some(format!("{error}"));
						set_window_visibility(ctx, true);
					}
				}
			}
		} else if self.auth.entitlements.is_some() {
			self.auth.error = None;
			self.auth.entitlements = None;
			let _ = self.auth.etx.send(None);
		}

		if self.auth.entitlements.is_some()
			&& self.auth.error.is_none()
			&& let Some(session_config) = settings.session
		{
			egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
				MenuBar::new().ui(ui, |ui| {
					ui.menu_button("Tacto", |ui| {
						if session_config.is_guest {
							ui.label("Guest Session");
						} else {
							ui.label(session_config.email);
						}
						ui.add_space(4.0);

						if !session_config.is_guest && ui.button("Open Tacto").clicked() {
							ctx.open_url(egui::OpenUrl::new_tab(format!(
								"{}/plugin_auth?userId={}&pluginSessionUuid={}",
								crate::ORIGIN,
								session_config.user_id,
								session_config.session_uuid
							)));
						}

						if ui.button("Log out").clicked() {
							if session_config.is_guest {
								// Guest sessions just clear local settings
								let mut settings = get_settings();
								settings.session = None;
								let _ = crate::set_settings(&settings);
								self.auth.entitlements = None;
								let _ = self.auth.etx.send(None);
							} else if let Err(error) =
								log_out(&session_config.user_id, &session_config.session_uuid)
							{
								self.auth.error = Some(format!("{error}"));
							} else {
								self.auth.error = None;
								self.auth.entitlements = None;
								let _ = self.auth.etx.send(None);
							}
						}
					});

					ui.with_layout(Layout::right_to_left(Default::default()), |ui| {
						ComboBox::from_id_salt("device")
							.selected_text(self.selected_device)
							.show_ui(ui, |ui| {
								ui.selectable_value(&mut self.selected_device, "Mobile", "Mobile");
								ui.selectable_value(
									&mut self.selected_device,
									"Keyboard",
									"Keyboard",
								);
							});

						if self.selected_device == "Keyboard" {
							ui.add_space(8.0);
							let mut enabled = self.keyboard.enabled;
							if ui.checkbox(&mut enabled, "Enabled").changed() {
								self.keyboard.enabled = enabled;
								let mut settings = get_settings();
								settings.keyboard_enabled = enabled;
								if let Err(e) = crate::set_settings(&settings) {
									log::error!("[keyboard] Failed to save settings: {e}");
								}
								// Trigger device registration/deregistration
								let _ = self.auth.etx.send(self.auth.entitlements.clone());
							}
						}
					});
				});
			});
		}

		egui::CentralPanel::default().show(ctx, |ui| {
			let show_login_ui = self.auth.entitlements.is_none() || self.auth.error.is_some();

			let size = if !show_login_ui && settings.keyboard_enabled {
				vec2(
					((114 + 12) * COLUMNS as usize + 4) as f32,
					((114 + 12) * ROWS as usize + 26) as f32,
				)
			} else {
				vec2(600.0, 400.0)
			};
			self.window_state.set_min_size(ctx, size);
			self.window_state.set_max_size(ctx, size);
			let rect = ctx.content_rect();
			if rect.width() < size.x
				|| rect.height() < size.y
				|| rect.width() > size.x
				|| rect.height() > size.y
			{
				ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
			}

			if show_login_ui {
				self.login_ui(ctx, ui);
			} else {
				match self.selected_device {
					"Mobile" => self.mobile_ui(ctx, ui),
					"Keyboard" => self.keyboard_ui(ctx, ui),
					_ => unreachable!(),
				}
			}
		});
	}
}

impl App {
	fn login_ui(&mut self, ctx: &Context, ui: &mut Ui) {
		ui.style_mut().spacing.item_spacing = vec2(6.0, 6.0);
		ui.style_mut().spacing.interact_size = vec2(20.0, 20.0);

		// Check for QR token creation result
		self.auth.qr.check_token_result(ctx);

		// Check for QR poll result
		if let Some(poll_result) = self.auth.qr.check_poll_result() {
			match poll_result {
				QrPollResult::Approved {
					user_id,
					session_uuid,
					email,
				} => {
					self.auth.pending_qr_result = Some(QrPollResult::Approved {
						user_id,
						session_uuid,
						email,
					});
					self.auth.error = None;
					self.auth.qr.reset();
				}
				QrPollResult::GuestPaired { session_uuid } => {
					self.auth.pending_qr_result = Some(QrPollResult::GuestPaired { session_uuid });
					self.auth.error = None;
					self.auth.qr.reset();
				}
				QrPollResult::Pending => {}
				QrPollResult::Error(msg) => {
					if msg.contains("expired") || msg.contains("not found") {
						self.auth.qr.token = None;
						self.auth.qr.texture = None;
						self.auth.qr.last_poll = None;
					}
				}
			}
		}

		// Initialize QR token if not present (non-blocking)
		if !self.auth.show_email_login {
			self.auth.qr.start_token_creation(ctx);
			self.auth.qr.check_expiration();
			self.auth.qr.start_poll(ctx);
			ctx.request_repaint_after(std::time::Duration::from_secs(2));
		}

		self.auth.last_content_height = ui
			.vertical_centered_justified(|ui| {
				ui.add_space((ui.available_height() - self.auth.last_content_height) / 1.25);

				ui.heading("Tacto");
				ui.add_space(4.0);

				if let Some(qr_result) = self.auth.pending_qr_result.clone() {
					ui.label(RichText::new("Did you scan the QR code yourself?").strong());
					ui.add_space(4.0);
					ui.label("Only confirm if you personally scanned the QR code. If someone else scanned it, they could control your system.");
					ui.add_space(8.0);

					if ui.button("Yes, I scanned it").clicked() {
						self.auth.pending_qr_result = None;
						match qr_result {
							QrPollResult::Approved { user_id, session_uuid, email } => {
								if let Err(e) = complete_qr_login(email, user_id.clone(), session_uuid.clone()) {
									self.auth.error = Some(format!("{e}"));
								} else {
									self.auth.error = None;
									self.auth.entitlements = get_settings()
										.session
										.and_then(|s| get_entitlements(&s.user_id, &s.session_uuid).ok());
									let _ = self.auth.etx.send(self.auth.entitlements.clone());
								}
							}
							QrPollResult::GuestPaired { session_uuid } => {
								if let Err(e) = complete_guest_login(session_uuid) {
									self.auth.error = Some(format!("{e}"));
								} else {
									self.auth.error = None;
									self.auth.entitlements = Some(vec![]);
									let _ = self.auth.etx.send(self.auth.entitlements.clone());
								}
							}
							_ => {}
						}
					}

					if ui.button("Cancel").clicked() {
						self.auth.pending_qr_result = None;
						self.auth.error = Some("Login cancelled.".to_string());
					}
				} else if self.auth.show_email_login {
					ui.add(TextEdit::singleline(&mut self.auth.email).hint_text("Email"));
					let response = ui.add(
						TextEdit::singleline(&mut self.auth.password)
							.password(true)
							.hint_text("Password"),
					);
					ui.add_space(4.0);

					if ui.button("Log in").clicked()
						|| (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
					{
						match log_in(&self.auth.email, &self.auth.password) {
							Ok(_) => {
								self.auth.error = None;
								self.auth.email = String::new();
								self.auth.password = String::new();
								self.auth.show_email_login = false;
								self.auth.entitlements =
									get_settings().session.and_then(|session| {
										get_entitlements(&session.user_id, &session.session_uuid)
											.ok()
									});
								let _ = self.auth.etx.send(self.auth.entitlements.clone());
							}
							Err(error) => self.auth.error = Some(format!("{error}")),
						}
					}

					ui.add_space(4.0);
					if ui.button("Log in with QR code instead").clicked() {
						self.auth.show_email_login = false;
						self.auth.error = None;
						self.auth.qr.reset();
					}
				} else {
					self.auth.qr.render(ui);

					ui.add_space(8.0);
					if ui.button("Log in with email instead").clicked() {
						self.auth.show_email_login = true;
						self.auth.error = None;
					}
				}

				if let Some(error) = &self.auth.error {
					ui.add_space(4.0);
					ui.label(RichText::new(error).color(Color32::LIGHT_RED));
				}
			})
			.response
			.rect
			.height();
	}

	fn mobile_ui(&mut self, _ctx: &Context, ui: &mut Ui) {
		let settings = get_settings();
		let is_guest = settings.session.as_ref().is_some_and(|s| s.is_guest);

		ui.vertical_centered_justified(|ui| {
			if crate::mobile::CONNECTED.load(std::sync::atomic::Ordering::Relaxed) {
				ui.heading("Connected to Tacto Mobile");
				ui.label(
					"You're set! Switch back to OpenDeck or Tacto Desktop\n to install plugins, create profiles and lay out your actions.",
				);
			} else {
				ui.heading("Ready to connect to Tacto Mobile");
				ui.label(format!(
					"Start by heading to {}/\n on your phone or tablet to get connected.",
					crate::ORIGIN
				));
			}
			ui.add_space(8.0);

			if is_guest {
				ui.label(
					"Guest session: create an account for automatic pairing and premium features.",
				);
			} else {
				ui.label(format!("Logged in as {}", settings.session.unwrap().email));
			}
			ui.label("Switch to Tacto Keyboard using the menu above.");
		});
	}
}
