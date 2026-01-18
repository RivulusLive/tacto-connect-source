// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

use crate::get_settings;
use crate::keyboard::{COLUMNS, ROWS, add_mapping, remove_mapping};

use std::collections::HashMap;
use std::sync::Mutex;

use eframe::egui::{
	self, Align, AtomExt, Button, Color32, ComboBox, Context, Id, Image, Layout, Rect, RichText,
	ScrollArea, Sense, Separator, Stroke, TextureHandle, Ui, Window, vec2,
};

use futures::executor::block_on;

static WINDOW_OPEN: Mutex<bool> = Mutex::new(false);

impl super::App {
	fn button(&mut self, ctx: &Context, ui: &mut Ui, rect: Rect, position: u8) {
		let disabled = position >= 6
			&& !self
				.auth
				.entitlements
				.as_ref()
				.unwrap()
				.iter()
				.any(|e| e == "keyboardUnlimited");

		let painter = ui.painter();
		painter.rect_stroke(
			rect,
			4.0,
			Stroke::new(
				1.5,
				if self.keyboard.selected_position == Some(position) {
					Color32::GRAY
				} else if !disabled {
					Color32::DARK_GRAY
				} else {
					Color32::BLACK
				},
			),
			egui::StrokeKind::Outside,
		);

		if let Some(texture) = ctx.data_mut(|d| d.get_temp::<TextureHandle>(Id::new(position))) {
			let mut image = Image::new(&texture).corner_radius(4.0);
			if disabled {
				image = image.tint(Color32::DARK_GRAY);
			}
			image.paint_at(ui, rect);
		}
	}

	fn window(&mut self, ctx: &Context, ui: &mut Ui) {
		ui.style_mut().spacing.item_spacing = vec2(8.0, 8.0);
		ui.style_mut().spacing.interact_size = vec2(30.0, 30.0);

		ui.horizontal(|ui| {
			ui.vertical(|ui| {
				if !self.keyboard.errors.is_empty() {
					ui.heading("Errors");
					ui.add_space(4.0);

					ScrollArea::vertical()
						.id_salt("errors")
						.max_width(ui.available_width() - 75.0)
						.max_height(120.0)
						.show(ui, |ui| {
							for error in self.keyboard.errors.iter() {
								ui.label(format!("{error}"));
							}
						});

					if ui.button("Clear errors").clicked() {
						self.keyboard.errors = Vec::new();
					}

					ui.add_sized([200.0, 0.0], Separator::default());
				}

				ui.heading("Add a mapping");
				ui.add_space(4.0);

				ui.horizontal(|ui| {
					let old = self.keyboard.selected_key;

					ComboBox::from_id_salt("key")
						.selected_text(if self.keyboard.selected_key.is_empty() {
							"Select a key..."
						} else {
							self.keyboard.selected_key
						})
						.show_ui(ui, |ui| {
							for key in crate::keyboard::KEYS
								.iter()
								.filter(|x| !self.keyboard.building_hot_key.contains(x))
							{
								ui.selectable_value(&mut self.keyboard.selected_key, key, *key);
							}
						});

					if self.keyboard.selected_key != old && !self.keyboard.selected_key.is_empty() {
						self.keyboard
							.building_hot_key
							.push(self.keyboard.selected_key);
						let key_order_map: HashMap<_, _> = crate::keyboard::KEYS
							.iter()
							.enumerate()
							.map(|(i, &v)| (v, i))
							.collect();
						self.keyboard
							.building_hot_key
							.sort_by_key(|x| key_order_map.get(x).unwrap());
						self.keyboard.selected_key = "";
					}
				});

				if !self.keyboard.building_hot_key.is_empty() {
					ui.add_space(4.0);

					ui.add(egui::AtomLayout::new(
						RichText::new(self.keyboard.building_hot_key.join("+"))
							.atom_max_width(ui.available_width() - 75.0),
					));
					ui.horizontal(|ui| {
						if ui
							.button(format!(
								"Map to button {}",
								self.keyboard.selected_position.unwrap()
							))
							.clicked()
						{
							if let Err(error) = block_on(add_mapping(
								&self.keyboard.manager,
								self.keyboard.building_hot_key.join(" + "),
								self.keyboard.selected_position.unwrap(),
							)) {
								if !self
									.keyboard
									.errors
									.iter()
									.any(|x| format!("{x}") == format!("{error}"))
								{
									self.keyboard.errors.push(error);
								}
							} else {
								self.keyboard.building_hot_key = Vec::new();
							}
						}

						if ui.button("Cancel").clicked() {
							self.keyboard.selected_key = "";
							self.keyboard.building_hot_key = Vec::new();
						}
					});
				}

				let mappings = get_settings().hot_key_mappings;
				if mappings
					.iter()
					.any(|x| x.1.contains(&self.keyboard.selected_position.unwrap()))
				{
					ui.add_sized([200.0, 0.0], Separator::default());

					ui.heading("Added mappings");
					ui.add_space(4.0);

					ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
						let mut mappings = mappings
							.iter()
							.filter(|x| x.1.contains(&self.keyboard.selected_position.unwrap()))
							.collect::<Vec<_>>();
						mappings.sort_by_key(|x| x.0.id);
						for (hot_key, _) in mappings {
							if ui
								.add_sized(
									[30.0, 30.0],
									Button::new(hot_key.to_string().to_uppercase()),
								)
								.clicked() && let Err(error) = block_on(remove_mapping(
								&self.keyboard.manager,
								*hot_key,
								self.keyboard.selected_position.unwrap(),
							)) && !self
								.keyboard
								.errors
								.iter()
								.any(|x| format!("{x}") == format!("{error}"))
							{
								self.keyboard.errors.push(error);
							}
						}
					});
				}
			});

			ui.with_layout(Layout::top_down(Align::RIGHT), |ui| {
				ui.with_layout(Layout::right_to_left(Default::default()), |ui| {
					ui.add_space(0.0);
					let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::empty());
					self.button(ctx, ui, rect, self.keyboard.selected_position.unwrap());
				});
				ui.add_space(2.0);
				ui.label(
					RichText::new(format!(
						"Button {}",
						self.keyboard.selected_position.unwrap() + 1
					))
					.size(16.0),
				);
			});
		});

		ui.add_space(-6.0);
	}

	pub(super) fn keyboard_ui(&mut self, ctx: &Context, ui: &mut Ui) {
		if !self.keyboard.enabled {
			ui.vertical_centered_justified(|ui| {
				ui.heading("Tacto Keyboard is disabled");
				ui.label("Enable the checkbox in the menu bar above to use Tacto Keyboard.");
			});
			return;
		}

		#[cfg(target_os = "linux")]
		if super::is_wayland() {
			ui.vertical_centered_justified(|ui| {
				ui.heading("Wayland not supported");
				ui.label("Tacto Keyboard is not supported on Wayland.");
				ui.label("You can continue to use Tacto Mobile, or switch to an X11 session.");
			});
			return;
		}

		let mut window_open = WINDOW_OPEN.lock().unwrap();

		ui.style_mut().spacing.item_spacing = vec2(12.0, 12.0);

		if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
			*window_open = false;
		}
		if !*window_open {
			self.keyboard.selected_position = None;
		}

		for row in 0..ROWS {
			ui.horizontal(|ui| {
				for column in 0..COLUMNS {
					let (rect, response) =
						ui.allocate_exact_size(vec2(114.0, 114.0), Sense::click());
					self.button(ctx, ui, rect, row * COLUMNS + column);
					if response.clicked() {
						if row * COLUMNS + column >= 6
							&& !self
								.auth
								.entitlements
								.as_ref()
								.unwrap()
								.iter()
								.any(|e| e == "keyboardUnlimited")
						{
							if let Some(session_config) = get_settings().session {
								ctx.open_url(egui::OpenUrl::new_tab(format!(
									"{}/plugin_auth?userId={}&pluginSessionUuid={}",
									crate::ORIGIN,
									session_config.user_id,
									session_config.session_uuid
								)));
							}
						} else {
							self.keyboard.selected_position = Some(row * COLUMNS + column);
							*window_open = true;
						}
					}
				}
			});
		}

		if self.keyboard.selected_position.is_some() {
			Window::new("")
				.id("mappings".into())
				.min_width(285.0)
				.default_width(285.0)
				.open(&mut window_open)
				.collapsible(false)
				.show(ctx, |ui| self.window(ctx, ui));
		}
	}
}
