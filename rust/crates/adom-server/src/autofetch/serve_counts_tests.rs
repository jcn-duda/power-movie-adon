//! Contadores vivos do painel: `queueDepth`/`lots` refletem a PASSAGEM, não a
//! foto do boot; sem passagem (stub/prepared) a foto continua valendo.

use std::sync::Arc;

use adom_autofetch::v3::{QueueCandidate, QueueV3};
use adom_index::NativeKv;
use adom_sources::now_ms;
use tokio::sync::mpsc;

use super::super::fakes::no_factory;
use super::super::recheck::{self, Lot};
use super::super::worker::AutofetchHandle;
use super::super::Policy;
use super::{Control, Engine, Message};

const SEARCH: &str = "tt1234567:1:2";
const ADAPTER: &str = "alldebrid";
const ACCOUNT: &str = "conta-de-teste";

fn hash_of(n: u32) -> String {
    format!("{n:040x}")
}

/// Lote ainda NÃO vencido: o `due` não o roda, mas ele está VIVO e conta.
fn lot(now: i64) -> Lot {
    Lot {
        search_key: SEARCH.to_string(),
        adapter_id: ADAPTER.to_string(),
        account: ACCOUNT.to_string(),
        info_hashes: vec![hash_of(1)],
        created_at: now,
        ..Lot::default()
    }
}

fn queued(kv: &NativeKv, now: i64) {
    let candidate = QueueCandidate {
        info_hash: hash_of(2),
        quality: Some("1080p".to_string()),
        pool: Some("br".to_string()),
        br: Some(true),
        dubbed: Some(true),
        ..QueueCandidate::default()
    };
    QueueV3::new(kv)
        .write_queue(SEARCH, &[candidate], 600, 6, Some((ADAPTER, ACCOUNT)), now)
        .unwrap();
}

/// (d) A foto do boot é só o FALLBACK: a passagem mede fila e lotes no fim e o
/// `status()` passa a devolver o valor vivo — inclusive quando a fila esvazia.
#[tokio::test]
async fn contadores_refletem_a_passagem_e_nao_a_foto_do_boot() {
    let now = now_ms();
    let kv = Arc::new(NativeKv::open_memory().unwrap());
    // Boot: um lote vivo, nenhuma fila.
    recheck::write(&kv, &lot(now), Policy::fixture(), now).unwrap();
    let control = Arc::new(Control::new(false, 60_000));
    let handle = AutofetchHandle::stub_enabled(Arc::clone(&kv), None, Arc::clone(&control));
    let before = handle.status();
    assert_eq!(before.queue_depth, 0);
    assert_eq!(before.lots, 1);

    // A fila chega DEPOIS do boot: sem passagem, a foto continua valendo.
    queued(&kv, now);
    assert_eq!(handle.status().queue_depth, 0, "sem passagem, fica a foto");

    let factory = no_factory();
    let (tx, mut rx) = mpsc::channel::<Message>(4);
    drop(tx);
    let engine = Engine::new(&kv, Policy::fixture(), None, &factory, &control, None);
    engine.run_once(&mut rx, now).await;

    let after = handle.status();
    assert_eq!(after.queue_depth, 1, "fila viva substitui a foto do boot");
    assert_eq!(after.lots, 1, "lote vivo conta mesmo sem estar vencido");

    // Passagem seguinte com a fila vazia: o contador baixa de novo (não congela).
    QueueV3::new(&kv).drop_queue(SEARCH).unwrap();
    engine.run_once(&mut rx, now).await;
    assert_eq!(handle.status().queue_depth, 0);
    assert_eq!(handle.status().lots, 1);
}
