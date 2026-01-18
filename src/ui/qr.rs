// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

use crate::auth::{QrAuthToken, QrPollResult, create_qr_token, poll_qr_token};

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{
	Color32, ColorImage, Context, Image, RichText, TextureHandle, TextureOptions, Ui, vec2,
};
use qrcode::QrCode;

pub struct QrState {
	pub token: Option<QrAuthToken>,
	pub texture: Option<TextureHandle>,
	pub last_poll: Option<Instant>,
	pub loading: bool,
	pub token_rx: Option<mpsc::Receiver<Result<QrAuthToken, String>>>,
	pub poll_rx: Option<mpsc::Receiver<Result<QrPollResult, String>>>,
	pub error: Option<String>,
}

impl QrState {
	pub fn new() -> Self {
		Self {
			token: None,
			texture: None,
			last_poll: None,
			loading: false,
			token_rx: None,
			poll_rx: None,
			error: None,
		}
	}

	pub fn generate_texture(&mut self, ctx: &Context) {
		if let Some(ref token) = self.token {
			let qr_url = format!("{}/app/mobile?qr_token={}", crate::ORIGIN, token.token);
			if let Ok(qr) = QrCode::new(qr_url.as_bytes()) {
				let qr_image = qr
					.render::<image::Luma<u8>>()
					.min_dimensions(200, 200)
					.build();
				let width = qr_image.width() as usize;
				let height = qr_image.height() as usize;

				let rgb_pixels: Vec<u8> = qr_image
					.pixels()
					.flat_map(|p| {
						let v = p.0[0];
						[v, v, v]
					})
					.collect();

				let color_image = ColorImage::from_rgb([width, height], &rgb_pixels);

				self.texture =
					Some(ctx.load_texture("qr_code", color_image, TextureOptions::LINEAR));

				log::debug!("[auth] Generated QR code for token: {}", token.token);
			}
		}
	}

	pub fn check_token_result(&mut self, ctx: &Context) {
		if let Some(ref rx) = self.token_rx
			&& let Ok(result) = rx.try_recv()
		{
			self.token_rx = None;
			match result {
				Ok(token) => {
					self.token = Some(token);
					self.loading = false;
					self.error = None;
					self.generate_texture(ctx);
				}
				Err(e) => {
					self.loading = false;
					self.error = Some(format!("QR login unavailable: {e}"));
				}
			}
		}
	}

	pub fn check_poll_result(&mut self) -> Option<QrPollResult> {
		if let Some(ref rx) = self.poll_rx
			&& let Ok(result) = rx.try_recv()
		{
			self.poll_rx = None;
			match result {
				Ok(poll_result) => {
					return Some(poll_result);
				}
				Err(e) => {
					log::error!("[auth] Failed to poll QR token: {e}");
				}
			}
		}
		None
	}

	pub fn check_expiration(&mut self) {
		if let Some(ref token) = self.token {
			let now = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_millis() as u64;
			if now >= token.expires {
				self.token = None;
				self.texture = None;
				self.last_poll = None;
			}
		}
	}

	pub fn should_poll(&mut self) -> bool {
		self.last_poll
			.map(|t| t.elapsed() >= Duration::from_secs(2))
			.unwrap_or(true)
	}

	pub fn start_poll(&mut self, ctx: &Context) {
		if self.token.is_some() && self.poll_rx.is_none() && self.should_poll() {
			self.last_poll = Some(Instant::now());
			if let Some(ref token) = self.token {
				let (tx, rx) = mpsc::channel();
				self.poll_rx = Some(rx);
				let token_str = token.token.clone();
				let ctx_clone = ctx.clone();
				std::thread::spawn(move || {
					let result = poll_qr_token(&token_str).map_err(|e| e.to_string());
					let _ = tx.send(result);
					ctx_clone.request_repaint();
				});
			}
		}
	}

	pub fn start_token_creation(&mut self, ctx: &Context) {
		if self.token.is_none() && !self.loading && self.token_rx.is_none() {
			self.loading = true;
			let (tx, rx) = mpsc::channel();
			self.token_rx = Some(rx);
			let ctx_clone = ctx.clone();
			std::thread::spawn(move || {
				let result = create_qr_token().map_err(|e| e.to_string());
				let _ = tx.send(result);
				ctx_clone.request_repaint();
			});
		}
	}

	pub fn reset(&mut self) {
		self.token = None;
		self.texture = None;
		self.last_poll = None;
		self.loading = false;
		self.token_rx = None;
		self.poll_rx = None;
		self.error = None;
	}

	pub fn render(&self, ui: &mut Ui) {
		ui.label("Scan with Tacto on your mobile device");
		ui.add_space(8.0);

		if self.loading {
			ui.label("Loading...");
		} else if let Some(ref texture) = self.texture {
			let size = vec2(180.0, 180.0);
			ui.add(Image::new(texture).fit_to_exact_size(size));
		}

		ui.add_space(8.0);

		if let Some(ref error) = self.error {
			ui.label(RichText::new(error).color(Color32::LIGHT_RED));
		}
	}
}
