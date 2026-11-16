//! Controle do operador: régua pura e o tick obedecendo à pausa por site.

use std::sync::Arc;

use axum::routing::get;
use axum::Json;

use super::{ControlPatch, ControlState, CrawlControl};
use crate::motor::ingest::test_support::FakeIngest;
use crate::motor::r#loop::{CrawlMotor, TickOutcome};
use crate::motor::types::{
    CrawlResultStatus, CrawlWorkResult, MotorConfig, PageEngine, PageFuture, PageOutcome,
};
use crate::store::ops::{CrawlStore, UrlRow};

fn patch(json: &str) -> ControlPatch {
    serde_json::from_str(json).expect("patch válido")
}

#[test]
fn padrao_e_tudo_ligado_e_estado_vazio_vale_o_padrao() {
    let control = CrawlControl::default();
    assert_eq!(control.stopped(), None);
    assert!(!control.site_blocked("nerdfilmes", 0));
    let lido: ControlState = serde_json::from_str("{}").expect("estado vazio");
    assert_eq!(lido, ControlState::default(), "sem nada gravado: ligado");
}

#[test]
fn pausa_e_desligado_globais_param_tudo() {
    let control = CrawlControl::default();
    control.apply(&patch(r#"{"paused":true}"#)).unwrap();
    assert_eq!(control.stopped(), Some("pausado"));
    assert!(control.site_blocked("mico", 0));
    control
        .apply(&patch(r#"{"paused":false,"enabled":false}"#))
        .unwrap();
    assert_eq!(control.stopped(), Some("desligado"));
    control.apply(&patch(r#"{"enabled":null}"#)).unwrap();
    assert_eq!(control.stopped(), None, "null volta ao padrão (ligado)");
}

#[test]
fn pausa_do_site_so_bloqueia_ele_e_some_quando_volta_ao_padrao() {
    let control = CrawlControl::default();
    control
        .apply(&patch(r#"{"site":"mico","paused":true}"#))
        .unwrap();
    assert!(control.site_blocked("mico", 0));
    assert!(!control.site_blocked("nerdfilmes", 0));
    assert_eq!(control.blocked_sites(0), vec!["mico".to_string()]);
    let state = control
        .apply(&patch(r#"{"site":"mico","paused":false}"#))
        .unwrap();
    assert!(state.sites.is_empty(), "site sem override sai do mapa");
    control
        .apply(&patch(r#"{"site":"vacatorrent","enabled":false}"#))
        .unwrap();
    assert!(control.site_off("vacatorrent"));
    control
        .apply(&patch(r#"{"site":"vacatorrent","reset":true}"#))
        .unwrap();
    assert!(!control.site_off("vacatorrent"));
}

#[test]
fn teto_por_hora_do_site_e_ritmo_com_precedencia() {
    let control = CrawlControl::default();
    control
        .apply(&patch(
            r#"{"site":"nerdfilmes","maxPerHour":2,"delayMs":5000}"#,
        ))
        .unwrap();
    control
        .apply(&patch(r#"{"delayMs":2000,"maxPerHour":50}"#))
        .unwrap();
    let now = 1_000_000;
    control.note_page("nerdfilmes", now);
    assert!(!control.site_blocked("nerdfilmes", now));
    control.note_page("nerdfilmes", now + 1);
    assert!(
        control.site_blocked("nerdfilmes", now + 2),
        "teto do site atingido"
    );
    assert!(
        !control.site_blocked("nerdfilmes", now + 3_600_001),
        "janela nova"
    );
    assert_eq!(
        control.delay_for("nerdfilmes", 1000),
        5000,
        "site vence o global"
    );
    assert_eq!(control.delay_for("mico", 1000), 2000, "global vence o .env");
    assert_eq!(control.global_cap(1500), 50);
    control.apply(&patch(r#"{"reset":true}"#)).unwrap();
    assert_eq!(control.snapshot(), ControlState::default());
}

#[test]
fn pedido_invalido_nao_muda_nada() {
    let control = CrawlControl::default();
    assert!(control.apply(&patch(r#"{"maxPerHour":0}"#)).is_err());
    assert!(control.apply(&patch(r#"{"delayMs":600001}"#)).is_err());
    assert!(control
        .apply(&patch(r#"{"site":" ","paused":true}"#))
        .is_err());
    assert!(serde_json::from_str::<ControlPatch>(r#"{"dryRun":true}"#).is_err());
    assert_eq!(control.snapshot(), ControlState::default());
}

// ── O tick obedece ──────────────────────────────────────────────────────────

struct EngineFake;

impl PageEngine for EngineFake {
    fn is_ready(&self) -> bool {
        true
    }

    fn execute<'a>(&'a self, row: &'a UrlRow) -> PageFuture<'a> {
        let mut work = CrawlWorkResult::parser_pending(row);
        work.status = CrawlResultStatus::Done;
        work.error = String::new();
        work.imdb = Some("tt0000001".to_string());
        Box::pin(async move { PageOutcome::Work(Box::new(work)) })
    }
}

async fn node_ocioso() -> String {
    let app = axum::Router::new().route(
        "/internal/activity",
        get(|| async { Json(serde_json::json!({ "recent": false })) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn semear(db_path: &str, site: &str, key: &str) {
    let db = CrawlStore::open(db_path).expect("abrir");
    db.migrate().expect("migrar");
    let row = UrlRow {
        site: site.to_string(),
        url: format!("https://exemplo.tv/{key}"),
        url_key: key.to_string(),
        kind: "movie".to_string(),
        status: "pending".to_string(),
        lastmod: String::new(),
        imdb: None,
        tries: 0,
        next_at: 0,
        checked_at: 0,
        releases: 0,
        error: String::new(),
        progress: String::new(),
        added_at: 0,
    };
    db.upsert_urls(site, &[row]).expect("semear");
}

#[tokio::test]
async fn site_pausado_fica_fora_do_lote_e_pausa_global_para_o_tick() {
    let dir = tempfile::tempdir().unwrap();
    let caminho = dir.path().join("crawl.db");
    let caminho = caminho.to_str().unwrap();
    semear(caminho, "nerdfilmes", "pausado-1");
    let node = node_ocioso().await;
    let motor = CrawlMotor::new(MotorConfig {
        db_path: caminho.to_string(),
        node_url: node,
        admin_token: "t".into(),
        idle_window_ms: 60_000,
        max_per_hour: 100,
        ..Default::default()
    })
    .with_page_engine(Arc::new(EngineFake))
    .with_ingest(Arc::new(FakeIngest::delivered()));

    motor
        .control()
        .apply(&patch(r#"{"site":"nerdfilmes","paused":true}"#))
        .unwrap();
    assert_eq!(
        motor.tick_once().await.unwrap(),
        TickOutcome::Skipped("sem-fila"),
        "o único site com fila está pausado"
    );
    motor.control().apply(&patch(r#"{"paused":true}"#)).unwrap();
    assert_eq!(
        motor.tick_once().await.unwrap(),
        TickOutcome::Skipped("pausado")
    );
    motor.control().apply(&patch(r#"{"reset":true}"#)).unwrap();
    assert!(matches!(
        motor.tick_once().await.unwrap(),
        TickOutcome::Processed { claimed: 1, .. }
    ));
}
