//! Relógio do lote: qual INTERVALO vale em cada modo.
//!
//! O Node arma o lote normal com `autoFetchRecheckMs` (2 min) e o lote já em
//! settle com `autoFetchSettleMs` (15 min). Trocar um pelo outro passaria a
//! bater no debrid 7,5× mais vezes sem nenhum teste de status reclamar — a
//! diferença só aparece no RELÓGIO, e é isso que este arquivo prende.

use adom_sources::now_ms;

use super::super::Policy;
use super::{due_at, Lot};

const SEARCH: &str = "tt1234567:1:2";

fn lot() -> Lot {
    Lot {
        search_key: SEARCH.to_string(),
        adapter_id: "alldebrid".to_string(),
        account: "conta-de-teste".to_string(),
        info_hashes: vec![format!("{:040x}", 1)],
        created_at: now_ms(),
        ..Lot::default()
    }
}

/// Cada ESCRITA do lote usa o TTL do próprio lote, então o intervalo não pode
/// ser negativo: `attempts` é a contagem de passagens, não o relógio.
#[test]
fn due_at_usa_o_intervalo_do_modo() {
    let policy = Policy::fixture();
    assert!(policy.settle_interval_ms != policy.recheck_interval_ms);
    let mut lot = lot();
    lot.attempts = 1;
    lot.is_settle = false;
    assert_eq!(
        due_at(&lot, policy),
        lot.created_at + 2 * policy.recheck_interval_ms,
        "fase curta: a 2ª passagem sai um intervalo depois da 1ª"
    );
    lot.attempts = policy.recheck_max;
    lot.is_settle = true;
    assert_eq!(
        due_at(&lot, policy),
        lot.created_at
            + policy.recheck_max * policy.recheck_interval_ms
            + policy.settle_interval_ms,
        "settle: um intervalo de settle a partir do fim da fase curta"
    );
}

/// Regressão medida na VPS (2026-11-15): o lote entrava em settle com
/// `attempts = 3` e a 1ª passagem dele caía em `createdAt + 3·settle` (45 min)
/// — o `armRecheck` do Node rearmava 15 min depois da última passagem curta.
#[test]
fn settle_conta_do_fim_da_fase_curta_e_nao_da_criacao() {
    let policy = Policy::fixture();
    let mut lot = lot();
    lot.is_settle = true;
    lot.attempts = policy.recheck_max;
    let short_end = lot.created_at + policy.recheck_max * policy.recheck_interval_ms;
    assert_eq!(due_at(&lot, policy), short_end + policy.settle_interval_ms);
    assert!(
        due_at(&lot, policy) < lot.created_at + policy.recheck_max * policy.settle_interval_ms,
        "a 1ª passagem em settle não pode esperar recheckMax intervalos de settle"
    );
    lot.attempts = policy.recheck_max + 1;
    assert_eq!(
        due_at(&lot, policy),
        short_end + 2 * policy.settle_interval_ms
    );
}

/// No teto da fase curta o prazo para de crescer: o tick seguinte entra em
/// settle sem esperar mais um intervalo curto à toa.
#[test]
fn fase_curta_no_teto_nao_empurra_o_prazo() {
    let policy = Policy::fixture();
    let mut lot = lot();
    lot.attempts = policy.recheck_max;
    assert_eq!(
        due_at(&lot, policy),
        lot.created_at + policy.recheck_max * policy.recheck_interval_ms
    );
}

/// Zero passagens ainda espera UM intervalo: sem isso o laço reprocessaria no
/// mesmo ciclo do enfileiramento.
#[test]
fn due_at_sem_passagem_espera_o_primeiro_intervalo() {
    let policy = Policy::fixture();
    let mut lot = lot();
    lot.attempts = 0;
    assert_eq!(
        due_at(&lot, policy),
        lot.created_at + policy.recheck_interval_ms
    );
}

/// O intervalo vem da POLÍTICA, não de uma const: mudar o knob muda o relógio.
#[test]
fn due_at_respeita_o_knob_do_settle() {
    let mut policy = Policy::fixture();
    policy.settle_interval_ms = 60_000;
    let mut lot = lot();
    lot.attempts = 3;
    lot.is_settle = true;
    assert_eq!(
        due_at(&lot, policy),
        lot.created_at + 3 * policy.recheck_interval_ms + 60_000
    );
}
