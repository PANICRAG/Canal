//! GET metrics handler.

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;

use devtools_core::filter::MetricsFilter;

use crate::error::ApiError;
use crate::state::AppState;

/// GET /v1/metrics/summary — aggregated metrics.
pub async fn get_metrics_summary(
    State(state): State<Arc<AppState>>,
    Query(filter): Query<MetricsFilter>,
) -> Result<impl IntoResponse, ApiError> {
    let metrics = state
        .devtools
        .get_metrics(filter)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(metrics))
}
