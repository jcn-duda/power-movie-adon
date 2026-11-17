//! DVD que tocou com partes apaga o `bad` herdado do `no_video` antigo; play
//! comum continua sem ressuscitar `bad` (regra do store).

use super::*;
use adom_index::MagnetState;
use serde_json::json;

const HASH: &str = "1358d48a5e4870166efa4fe524372dea9a55f395";

fn fresh() -> (tempfile::TempDir, NativePlayEffects) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = EffectConfig {
        magnet_db: true,
        alive_ttl: 7 * 24 * 3600,
        bad_ttl: 24 * 3600,
        lie_enabled: true,
        lie_ttl: 7 * 24 * 3600,
        avail_neg_ttl: 120,
        rd_ledger: true,
        rd_blocked_ttl: 2_592_000.0,
        blocked_cooldown_ms: 30_000,
    };
    let fx = NativePlayEffects::open_at(cfg, &dir.path().join("cache.db"), None);
    (dir, fx)
}

fn job(disc: bool) -> PlayJob {
    PlayJob {
        service: "torbox".into(),
        api_key: "chave-de-teste".into(),
        hash: HASH.into(),
        imdb: "tt0097972".into(),
        season: None,
        episode: None,
        disc,
    }
}

fn state(fx: &NativePlayEffects) -> Option<MagnetState> {
    fx.mag
        .as_ref()
        .expect("mag aberto")
        .state("torbox", "chave-de-teste", HASH, now_ms())
        .expect("leitura")
}

fn no_video() -> serde_json::Value {
    json!({ "status": 404, "kind": "no_video", "body": "x", "mark": "bad",
        "noteBlocked": false, "unprotect": false, "invalidate": "obra", "index": null })
}

#[test]
fn dvd_com_partes_apaga_o_bad_do_no_video_antigo() {
    let (_dir, fx) = fresh();
    fx.apply_error(job(false), no_video(), None);
    assert_eq!(state(&fx), Some(MagnetState::Bad));
    fx.apply_ok(job(true), true, false, None);
    assert_eq!(state(&fx), Some(MagnetState::Alive));
}

#[test]
fn play_comum_nao_ressuscita_bad() {
    let (_dir, fx) = fresh();
    fx.apply_error(job(false), no_video(), None);
    fx.apply_ok(job(false), true, false, None);
    assert_eq!(state(&fx), Some(MagnetState::Bad));
}
