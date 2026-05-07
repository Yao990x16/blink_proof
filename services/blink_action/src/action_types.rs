use axum::{
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ActionGetResponse {
    #[serde(rename = "type")]
    pub action_type: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub label: &'static str,
}

#[derive(Deserialize)]
pub struct ActionPostRequest {
    pub account: Option<String>,
    pub image_url: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ActionPostQuery {
    pub account: Option<String>,
    pub image_url: Option<String>,
}

#[derive(Serialize)]
pub struct ActionPostResponse {
    pub transaction: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct ActionError {
    pub message: String,
}

#[derive(Serialize)]
pub struct ActionsJsonResponse {
    pub rules: Vec<ActionRule>,
}

#[derive(Serialize)]
pub struct ActionRule {
    #[serde(rename = "pathPattern")]
    pub path_pattern: &'static str,
    #[serde(rename = "apiPath")]
    pub api_path: &'static str,
}

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    pub fn from_json_rejection(rejection: JsonRejection) -> Self {
        Self {
            status: rejection.status(),
            message: format!("invalid JSON payload: {rejection}"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ActionError {
            message: self.message,
        });
        (self.status, body).into_response()
    }
}
