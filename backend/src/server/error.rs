//! API 错误类型 / API error types

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub struct ApiError(pub String);

impl From<crate::core::error::AppError> for ApiError {
    fn from(e: crate::core::error::AppError) -> Self {
        ApiError(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.0).into_response()
    }
}

pub type ApiResult<T> = std::result::Result<Json<T>, ApiError>;
