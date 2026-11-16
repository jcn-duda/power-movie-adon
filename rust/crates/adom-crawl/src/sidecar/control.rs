//! `GET/POST /crawl/control` — os botões do painel chegam ao motor.
//!
//! O estado mora no [`CrawlControl`](crate::motor::control::CrawlControl) do
//! motor e é gravado no `crawl_state` a cada mudança (sobrevive ao restart).
//! Segurança: as mesmas regras de [`super::server`] (loopback + token).

use std::sync::Arc;

use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::server::{validate_token, AppState};
use crate::motor::control::{ControlPatch, ControlState, CONTROL_KEY, CONTROL_SITE};
use crate::motor::types::now_ms;
use crate::store::CrawlStore;

/// Estado gravado no `crawl_state`; sem nada (ou ilegível) vale o padrão.
pub fn load(store: &CrawlStore) -> ControlState {
    match store.get_state(CONTROL_SITE, CONTROL_KEY) {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            eprintln!("[adom-sidecar] controle ilegível ({e}); valendo o padrão");
            ControlState::default()
        }),
        Ok(None) => ControlState::default(),
        Err(e) => {
            eprintln!("[adom-sidecar] controle não lido ({e}); valendo o padrão");
            ControlState::default()
        }
    }
}

async fn body(state: &AppState, control: &ControlState) -> serde_json::Value {
    let now = now_ms();
    let motor = state.motor.control();
    let pages: serde_json::Map<String, serde_json::Value> = crate::site_profiles::all_site_ids()
        .into_iter()
        .map(|site| (site.to_string(), json!(motor.pages_this_hour(site, now))))
        .collect();
    let until = state.motor.auto_pause_until().await;
    let auto_pause = (until > now).then_some(until);
    json!({ "ok": true, "control": control, "pagesThisHour": pages, "autoPauseUntil": auto_pause })
}

/// `GET /crawl/control` — estado atual + páginas na hora de cada site.
pub async fn control_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = validate_token(&state.auth_token, &headers) {
        return resp;
    }
    let control = state.motor.control().snapshot();
    Json(body(&state, &control).await).into_response()
}

/// `POST /crawl/control` — aplica o pedido, grava e devolve o estado novo.
pub async fn control_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Result<Json<ControlPatch>, JsonRejection>,
) -> Response {
    if let Err(resp) = validate_token(&state.auth_token, &headers) {
        return resp;
    }
    let patch = match payload {
        Ok(Json(patch)) => patch,
        Err(e) => return bad_request(&e.body_text()),
    };
    if let Some(site) = patch.site.as_deref().map(str::trim) {
        if !crate::site_profiles::all_site_ids().contains(&site) {
            return bad_request(&format!("site desconhecido: {site}"));
        }
    }
    let control = state.motor.control();
    let before = control.snapshot();
    let next = match control.apply(&patch) {
        Ok(next) => next,
        Err(e) => return bad_request(&e),
    };
    let raw = match serde_json::to_string(&next) {
        Ok(raw) => raw,
        Err(e) => return internal(&before, &control, &format!("serializar: {e}")),
    };
    if let Err(e) = state
        .store
        .lock()
        .await
        .set_state(CONTROL_SITE, CONTROL_KEY, &raw)
    {
        // Sem gravar, o restart desfaria o botão: volta ao estado anterior.
        return internal(&before, &control, &format!("gravar: {e}"));
    }
    if patch.site.is_none() && patch.paused == Some(false) {
        state.motor.clear_auto_pause().await;
    }
    eprintln!("[adom-sidecar] controle do raspador: {raw}");
    Json(body(&state, &next).await).into_response()
}

fn internal(
    before: &ControlState,
    control: &crate::motor::control::CrawlControl,
    msg: &str,
) -> Response {
    control.replace(before.clone());
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": msg })),
    )
        .into_response()
}
