//! `GET/POST /crawl/control`: o botão chega ao motor e sobrevive ao restart.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::server::{router, AppState};
use crate::motor::types::MotorConfig;
use crate::motor::CrawlMotor;
use crate::store::CrawlStore;

const TOKEN: &str = "token-de-teste";

async fn subir(path: &str) -> (String, Arc<CrawlMotor>) {
    let store = CrawlStore::open(path).expect("abrir");
    store.migrate().expect("migrar");
    let motor = Arc::new(CrawlMotor::new(MotorConfig::default()));
    motor.control().replace(super::control::load(&store));
    let state = Arc::new(AppState {
        motor: motor.clone(),
        store: Arc::new(Mutex::new(store)),
        auth_token: TOKEN.to_string(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    (format!("http://{addr}"), motor)
}

async fn post(base: &str, body: Value, token: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/crawl/control"))
        .header("X-Indexer-Test-Token", token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn botao_chega_ao_motor_e_sobrevive_ao_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crawl.db");
    let path = path.to_str().unwrap();
    let (base, motor) = subir(path).await;

    let (status, body) = post(&base, json!({"site": "mico", "paused": true}), TOKEN).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["control"]["sites"]["mico"]["paused"], true);
    assert!(
        motor.control().site_blocked("mico", 0),
        "o motor obedece na hora"
    );
    assert_eq!(post(&base, json!({"maxPerHour": 40}), TOKEN).await.0, 200);

    let (status, _) = post(&base, json!({"paused": true}), "errado").await;
    assert_eq!(status, 401);
    let (status, _) = post(&base, json!({"site": "inexistente", "paused": true}), TOKEN).await;
    assert_eq!(status, 400);
    let (status, _) = post(&base, json!({"dryRun": true}), TOKEN).await;
    assert_eq!(status, 400, "dryRun não existe no sidecar");

    // "Restart": outro processo sobre o mesmo crawl.db relê o estado.
    let (base2, motor2) = subir(path).await;
    assert!(motor2.control().site_off("mico"));
    assert_eq!(motor2.control().global_cap(1500), 40);
    let got: Value = reqwest::Client::new()
        .get(format!("{base2}/crawl/control"))
        .header("X-Indexer-Test-Token", TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["control"]["maxPerHour"], 40);
    assert!(got["pagesThisHour"]["mico"].is_number());
}

#[tokio::test]
async fn retomar_global_tira_a_auto_pausa() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crawl.db");
    let (base, motor) = subir(path.to_str().unwrap()).await;
    for _ in 0..5 {
        motor.settle_budget_for_test().await;
    }
    assert!(
        motor.auto_pause_until().await > 0,
        "cinco lotes sem entrega auto-pausam"
    );
    let (_, body) = post(&base, json!({"site": "mico", "paused": false}), TOKEN).await;
    assert!(
        body["autoPauseUntil"].is_number(),
        "retomar SITE não mexe na auto-pausa"
    );
    let (status, body) = post(&base, json!({"paused": false}), TOKEN).await;
    assert_eq!(status, 200);
    assert!(body["autoPauseUntil"].is_null());
    assert_eq!(motor.auto_pause_until().await, 0);
}
