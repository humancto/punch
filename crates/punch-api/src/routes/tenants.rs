//! Tenant management endpoints for multi-tenant administration.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_types::{Tenant, TenantId, TenantQuota, TenantStatus};

use crate::AppState;

/// Build the tenant routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tenants", post(register_tenant).get(list_tenants))
        .route("/api/tenants/{id}", get(get_tenant).delete(delete_tenant))
        .route("/api/tenants/{id}/quota", axum::routing::put(update_quota))
        .route(
            "/api/tenants/{id}/suspend",
            post(suspend_tenant),
        )
        .route(
            "/api/tenants/{id}/activate",
            post(activate_tenant),
        )
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegisterTenantRequest {
    name: String,
    #[serde(default)]
    quota: Option<TenantQuota>,
}

#[derive(Serialize)]
struct TenantResponse {
    id: TenantId,
    name: String,
    api_key: String,
    status: TenantStatus,
    quota: TenantQuota,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl From<Tenant> for TenantResponse {
    fn from(t: Tenant) -> Self {
        Self {
            id: t.id,
            name: t.name,
            api_key: t.api_key,
            status: t.status,
            quota: t.quota,
            created_at: t.created_at,
        }
    }
}

#[derive(Serialize)]
struct TenantSummary {
    id: TenantId,
    name: String,
    status: TenantStatus,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct MessageResponse {
    message: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/tenants -- register a new tenant.
#[instrument(skip_all)]
async fn register_tenant(
    State(state): State<AppState>,
    Json(body): Json<RegisterTenantRequest>,
) -> (StatusCode, Json<TenantResponse>) {
    let quota = body.quota.unwrap_or_default();
    let tenant = state.ring.tenant_registry().register_tenant(body.name, quota);

    (StatusCode::CREATED, Json(TenantResponse::from(tenant)))
}

/// GET /api/tenants -- list all tenants (admin only).
#[instrument(skip_all)]
async fn list_tenants(State(state): State<AppState>) -> Json<Vec<TenantSummary>> {
    let tenants = state.ring.tenant_registry().list_tenants();

    let summaries = tenants
        .into_iter()
        .map(|t| TenantSummary {
            id: t.id,
            name: t.name,
            status: t.status,
        })
        .collect();

    Json(summaries)
}

/// GET /api/tenants/:id -- get tenant details.
#[instrument(skip(state))]
async fn get_tenant(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<TenantResponse>, (StatusCode, Json<ErrorResponse>)> {
    let tenant_id = TenantId(id);
    let tenant = state
        .ring
        .tenant_registry()
        .get_tenant(&tenant_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("tenant {} not found", id),
                }),
            )
        })?;

    Ok(Json(TenantResponse::from(tenant)))
}

/// PUT /api/tenants/:id/quota -- update tenant quota.
#[instrument(skip(state, body))]
async fn update_quota(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<TenantQuota>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let tenant_id = TenantId(id);
    state
        .ring
        .tenant_registry()
        .update_quota(&tenant_id, body)
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(MessageResponse {
        message: format!("quota updated for tenant {}", id),
    }))
}

/// POST /api/tenants/:id/suspend -- suspend a tenant.
#[instrument(skip(state))]
async fn suspend_tenant(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let tenant_id = TenantId(id);
    state
        .ring
        .tenant_registry()
        .suspend_tenant(&tenant_id)
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(MessageResponse {
        message: format!("tenant {} suspended", id),
    }))
}

/// POST /api/tenants/:id/activate -- activate a tenant.
#[instrument(skip(state))]
async fn activate_tenant(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let tenant_id = TenantId(id);
    state
        .ring
        .tenant_registry()
        .activate_tenant(&tenant_id)
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(MessageResponse {
        message: format!("tenant {} activated", id),
    }))
}

/// DELETE /api/tenants/:id -- delete a tenant.
#[instrument(skip(state))]
async fn delete_tenant(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let tenant_id = TenantId(id);

    // Remove all fighters belonging to this tenant.
    let tenant_fighters = state.ring.list_fighters_for_tenant(&tenant_id);
    for (fighter_id, _, _) in &tenant_fighters {
        state.ring.kill_fighter(fighter_id);
    }

    state
        .ring
        .tenant_registry()
        .delete_tenant(&tenant_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("tenant {} not found", id),
                }),
            )
        })?;

    Ok(StatusCode::NO_CONTENT)
}
