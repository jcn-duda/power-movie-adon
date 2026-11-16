//! Relógio e rodízio da descoberta dentro do tick (fase 6).
//!
//! Sem esta costura o motor drenava a fila e parava em `sem-fila` para sempre —
//! a fila `crawl_url` é ALIMENTADA pela descoberta. O contrato medido aqui: um
//! site por tick com rodízio, relógio por site (completa = intervalo cheio,
//! parcial/erro = retry curto) e nenhum site órfão depois de erro.
//!
//! Custo, cursores e a costura com a fila ficam em
//! `discovery_tick_state_tests`; o harness é `discovery_tick_test_support`.

use crate::discovery::discoverable_sites;
use crate::motor::r#loop::CrawlMotor;
use crate::motor::types::now_ms;
use crate::store::{CrawlStore, UrlRow};

use super::test_support::*;

// ─── Rodízio e relógio ──────────────────────────────────────────────────────

#[tokio::test]
async fn um_site_por_tick_e_rodizio_completo() {
    let (_dir, db) = db_temp();
    let fake = FakeDiscovery::new(Vec::new());
    let motor = motor(&db, "http://127.0.0.1:1", fake.clone());

    for _ in 0..discoverable_sites().len() {
        assert!(
            motor.discover_once().await.is_some(),
            "cada tick descobre UM site"
        );
    }
    let mut rodizio: Vec<String> = fake.calls().iter().map(|c| c.site.clone()).collect();
    rodizio.sort();
    let mut esperado: Vec<String> = discoverable_sites().iter().map(|s| s.to_string()).collect();
    esperado.sort();
    assert_eq!(rodizio, esperado, "todo site vencido passou uma vez");

    assert!(
        motor.discover_once().await.is_none(),
        "nenhum site vencido: nada a fazer"
    );
}

#[tokio::test]
async fn parcial_volta_no_retry_curto_e_completa_no_intervalo() {
    let (_dir, db) = db_temp();
    let fake = FakeDiscovery::new(vec![Ok(report_parcial())]);
    let motor = motor(&db, "http://127.0.0.1:1", fake);

    let antes = now_ms();
    let parcial = motor.discover_once().await.expect("parcial");
    let alvo = motor
        .discover_next_at
        .lock()
        .await
        .get(&parcial.site)
        .copied()
        .expect("site agendado");
    assert!(
        alvo - antes <= 7_000,
        "parcial volta no prazo curto de retry ({} ms)",
        alvo - antes
    );

    motor.force_discovery().await;
    let antes = now_ms();
    let completa = motor.discover_once().await.expect("completa");
    let alvo = motor
        .discover_next_at
        .lock()
        .await
        .get(&completa.site)
        .copied()
        .expect("site agendado");
    assert!(
        alvo - antes >= 3_500_000,
        "rodada completa espera o intervalo cheio ({} ms)",
        alvo - antes
    );
}

#[tokio::test]
async fn erro_do_site_nao_derruba_o_tick() {
    let (_dir, db) = db_temp();
    let fake = FakeDiscovery::new(vec![Err("índice ilegível".to_string())]);
    let motor = motor(&db, "http://127.0.0.1:1", fake);

    assert!(
        motor.discover_once().await.is_none(),
        "rodada sem resultado não vira tick"
    );
    let alvo = motor
        .discover_next_at
        .lock()
        .await
        .get(PRIMEIRO)
        .copied()
        .expect("o site volta no retry, não fica órfão");
    assert!(alvo > 0);
    assert_eq!(
        motor.status().await.pages_this_hour,
        0,
        "erro antes do fetch de obra não cobra"
    );
}

#[tokio::test]
async fn sem_descoberta_o_motor_nao_descobre() {
    let (_dir, db) = db_temp();
    let motor = CrawlMotor::new(config(&db, "http://127.0.0.1:1"));
    assert!(!motor.discovery_ready());
    assert!(motor.discover_once().await.is_none());
}

// ─── Revisita de no-torrent antigos (knob default OFF) ──────────────────────

/// Semeia uma obra `no-torrent` lida há 40 dias no primeiro site do rodízio.
fn semear_no_torrent_antigo(db: &str) {
    let store = CrawlStore::open(db).expect("abrir");
    store.migrate().expect("migrar");
    let row = UrlRow {
        site: PRIMEIRO.to_string(),
        url: "https://vaqueirofilmes.com/antiga/".to_string(),
        url_key: "/antiga/".to_string(),
        kind: "movie".to_string(),
        status: "no-torrent".to_string(),
        lastmod: "2026-09-01T00:00:00Z".to_string(),
        imdb: None,
        tries: 0,
        next_at: 0,
        checked_at: now_ms() - 40 * 24 * 3_600_000,
        releases: 0,
        error: String::new(),
        progress: String::new(),
        added_at: 0,
    };
    store.upsert_urls(PRIMEIRO, &[row]).expect("semear");
}

fn status_da_antiga(db: &str) -> String {
    let store = CrawlStore::open(db).expect("reabrir");
    store
        .conn()
        .query_row(
            "SELECT status FROM crawl_url WHERE site = ?1 AND url_key = '/antiga/'",
            [PRIMEIRO],
            |r| r.get(0),
        )
        .expect("linha")
}

/// Default 0 = DESLIGADO (medido sem ganho em 2026-11-15): a rodada de
/// descoberta não devolve o no-torrent antigo à fila.
#[tokio::test]
async fn revisita_de_no_torrent_e_desligada_por_default() {
    let (_dir, db) = db_temp();
    semear_no_torrent_antigo(&db);
    let fake = FakeDiscovery::new(Vec::new());
    let motor = motor(&db, "http://127.0.0.1:1", fake);
    motor.discover_once().await.expect("rodada");
    assert_eq!(
        status_da_antiga(&db),
        "no-torrent",
        "sem knob, sem revisita"
    );
}

/// Ligada, a revisita roda DEPOIS de a fila aceitar a rodada — até rodada
/// vazia a dispara (o corte incremental nunca devolve post antigo).
#[tokio::test]
async fn revisita_ligada_devolve_o_no_torrent_antigo_na_rodada() {
    let (_dir, db) = db_temp();
    semear_no_torrent_antigo(&db);
    let fake = FakeDiscovery::new(Vec::new());
    let mut cfg = config(&db, "http://127.0.0.1:1");
    cfg.revisit_no_torrent_days = 21;
    let motor = CrawlMotor::new(cfg).with_discovery(fake);
    motor.discover_once().await.expect("rodada");
    assert_eq!(
        status_da_antiga(&db),
        "pending",
        "a revisita devolveu à fila"
    );
}
