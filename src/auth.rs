// Licensed under the Rivulus Source Available Software License, Version 1.0.
// See the LICENSE file in the project root for license terms.

use crate::{ORIGIN, get_settings, set_settings};

use std::collections::HashMap;

use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Debug)]
pub enum AuthError {
	Auth(String),
	Network(reqwest::Error),
	IO(std::io::Error),
}
impl From<reqwest::Error> for AuthError {
	fn from(err: reqwest::Error) -> Self {
		AuthError::Network(err)
	}
}
impl From<std::io::Error> for AuthError {
	fn from(err: std::io::Error) -> Self {
		AuthError::IO(err)
	}
}
impl std::fmt::Display for AuthError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			AuthError::Auth(msg) => write!(f, "Auth error: {}", msg),
			AuthError::Network(error) => write!(f, "Network error: {}", error),
			AuthError::IO(error) => write!(f, "I/O error: {}", error),
		}
	}
}
impl std::error::Error for AuthError {}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ApiResponse {
	#[serde(rename = "success")]
	Success { data: String },
	#[serde(rename = "error")]
	Error { error: ApiError },
}

#[derive(Debug, Deserialize)]
struct ApiError {
	message: String,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct LoginIndexMapping {
	userId: usize,
	pluginSessionUuid: usize,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct QrTokenIndexMapping {
	qrToken: usize,
	expires: usize,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct QrPollIndexMapping {
	status: usize,
	userId: Option<usize>,
	pluginSessionUuid: Option<usize>,
	email: Option<usize>,
	#[allow(dead_code)]
	isGuest: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct QrAuthToken {
	pub token: String,
	pub expires: u64,
}

#[derive(Debug, Clone)]
pub enum QrPollResult {
	Pending,
	Approved {
		user_id: String,
		session_uuid: String,
		email: String,
	},
	GuestPaired {
		session_uuid: String,
	},
	Error(String),
}

pub fn log_in(email: &str, password: &str) -> Result<(), AuthError> {
	let client = Client::new();

	let mut form = HashMap::new();
	form.insert("email", email);
	form.insert("password", password);

	let res = client
		.post(format!("{}/plugin_auth?/authenticate_user", ORIGIN))
		.header("Origin", ORIGIN)
		.form(&form)
		.send()?;

	let body = res.text()?;
	let parsed: ApiResponse = serde_json::from_str(&body).unwrap();

	match parsed {
		ApiResponse::Success { data } => {
			let inner: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
			let mapping: LoginIndexMapping = serde_json::from_value(inner[0].clone()).unwrap();

			let user_id = inner[mapping.userId].as_str().unwrap();
			let session_uuid = inner[mapping.pluginSessionUuid].as_str().unwrap();

			let mut settings = get_settings();
			settings.session = Some(crate::SessionConfig {
				email: email.to_owned(),
				user_id: user_id.to_owned(),
				session_uuid: session_uuid.to_owned(),
				is_guest: false,
			});
			set_settings(&settings)?;

			Ok(())
		}
		ApiResponse::Error { error } => Err(AuthError::Auth(error.message)),
	}
}

pub fn log_out(user_id: &str, session_uuid: &str) -> Result<(), AuthError> {
	let client = Client::new();

	let mut form = HashMap::new();
	form.insert("userId", user_id);
	form.insert("pluginSessionUuid", session_uuid);

	let res = client
		.post(format!("{}/plugin_auth?/sign_out", ORIGIN))
		.header("Origin", ORIGIN)
		.form(&form)
		.send()?;

	if !res.status().is_success() {
		Err(AuthError::Auth("Failed to log out".to_owned()))
	} else {
		let mut settings = get_settings();
		settings.session = None;
		set_settings(&settings)?;

		Ok(())
	}
}

pub fn get_entitlements(user_id: &str, session_uuid: &str) -> Result<Vec<String>, AuthError> {
	let client = Client::new();

	let mut form = HashMap::new();
	form.insert("userId", user_id);
	form.insert("pluginSessionUuid", session_uuid);

	let res = client
		.post(format!("{}/plugin_auth?/get_entitlements", ORIGIN))
		.header("Origin", ORIGIN)
		.form(&form)
		.send()?;

	let body = res.text()?;
	let parsed: ApiResponse = serde_json::from_str(&body).unwrap();

	match parsed {
		ApiResponse::Success { data } => {
			let inner: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();

			let mut entitlements = Vec::new();
			for i in inner[0].as_array().unwrap() {
				entitlements.push(
					inner[i.as_u64().unwrap() as usize]
						.as_str()
						.unwrap()
						.to_string(),
				);
			}

			Ok(entitlements)
		}
		ApiResponse::Error { error } => Err(AuthError::Auth(error.message)),
	}
}

pub fn create_qr_token() -> Result<QrAuthToken, AuthError> {
	let client = Client::new();

	let res = client
		.post(format!("{}/plugin_auth?/create_qr_token", ORIGIN))
		.header("Origin", ORIGIN)
		.form(&HashMap::<&str, &str>::new())
		.send()?;

	let body = res.text()?;
	let parsed: ApiResponse = serde_json::from_str(&body).unwrap();

	match parsed {
		ApiResponse::Success { data } => {
			let inner: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
			let mapping: QrTokenIndexMapping = serde_json::from_value(inner[0].clone()).unwrap();

			let token = inner[mapping.qrToken].as_str().unwrap().to_owned();
			let expires = inner[mapping.expires].as_u64().unwrap();

			Ok(QrAuthToken { token, expires })
		}
		ApiResponse::Error { error } => Err(AuthError::Auth(error.message)),
	}
}

pub fn poll_qr_token(qr_token: &str) -> Result<QrPollResult, AuthError> {
	let client = Client::new();

	let mut form = HashMap::new();
	form.insert("qrToken", qr_token);

	let res = client
		.post(format!("{}/plugin_auth?/poll_qr_token", ORIGIN))
		.header("Origin", ORIGIN)
		.form(&form)
		.send()?;

	let body = res.text()?;
	let parsed: ApiResponse = serde_json::from_str(&body).unwrap();

	match parsed {
		ApiResponse::Success { data } => {
			let inner: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
			let mapping: QrPollIndexMapping = serde_json::from_value(inner[0].clone()).unwrap();

			let status = inner[mapping.status].as_str().unwrap();

			if status == "approved" {
				let user_id = inner[mapping.userId.unwrap()].as_str().unwrap().to_owned();
				let session_uuid = inner[mapping.pluginSessionUuid.unwrap()]
					.as_str()
					.unwrap()
					.to_owned();
				let email = inner[mapping.email.unwrap()].as_str().unwrap().to_owned();

				Ok(QrPollResult::Approved {
					user_id,
					session_uuid,
					email,
				})
			} else if status == "guest_paired" {
				let session_uuid = inner[mapping.pluginSessionUuid.unwrap()]
					.as_str()
					.unwrap()
					.to_owned();

				Ok(QrPollResult::GuestPaired { session_uuid })
			} else {
				Ok(QrPollResult::Pending)
			}
		}
		ApiResponse::Error { error } => Ok(QrPollResult::Error(error.message)),
	}
}

pub fn complete_qr_login(
	email: String,
	user_id: String,
	session_uuid: String,
) -> Result<(), std::io::Error> {
	let mut settings = get_settings();
	settings.session = Some(crate::SessionConfig {
		email,
		user_id,
		session_uuid,
		is_guest: false,
	});
	set_settings(&settings)
}

pub fn complete_guest_login(session_uuid: String) -> Result<(), std::io::Error> {
	let mut settings = get_settings();
	settings.session = Some(crate::SessionConfig {
		email: "Guest".to_owned(),
		user_id: "guest".to_owned(),
		session_uuid,
		is_guest: true,
	});
	set_settings(&settings)
}
