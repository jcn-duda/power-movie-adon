//! Leitura em lote do `load`: UMA trava para o lote inteiro, forma ilegível
//! logada e pulada (não derruba os lotes sãos) e expiradas podadas na listagem.

use adom_sources::now_ms;
use serde_json::json;

use super::super::{load, lot_key, Policy, LOT_PREFIX};
use super::{hash_of, kv, lot, write, SEARCH};

/// JSON válido com forma errada não derruba o lote: é logado e pulado, e os
/// lotes sãos da MESMA leitura chegam juntos (o ganho é uma trava só).
#[test]
fn load_le_o_lote_numa_trava_e_pula_forma_ilegivel() {
    let kv = kv();
    let now = now_ms();
    write(&kv, &lot(SEARCH, &hash_of(1)), Policy::fixture(), now).unwrap();
    write(
        &kv,
        &lot("outra:busca", &hash_of(2)),
        Policy::fixture(),
        now,
    )
    .unwrap();
    kv.put_json(&lot_key("forma:ruim"), &json!({ "attempts": 1 }), 600, now)
        .unwrap();

    let lots = load(&kv, now).unwrap();
    assert_eq!(lots.len(), 2, "forma ruim não derruba os lotes sãos");
    assert!(lots.iter().any(|lot| lot.search_key == SEARCH));
    assert!(lots.iter().any(|lot| lot.search_key == "outra:busca"));
}

/// Expiradas saem na listagem (`keys_matching` poda): o efeito líquido é o
/// mesmo do `get_json` antigo, que apagava a vencida na leitura.
#[test]
fn load_poda_expiradas_na_listagem() {
    let kv = kv();
    let now = now_ms();
    let policy = Policy::fixture();
    write(&kv, &lot(SEARCH, &hash_of(1)), policy, now).unwrap();
    let later = now + policy.lot_ttl_seconds * 1000 + 1;
    assert!(load(&kv, later).unwrap().is_empty());
    assert!(
        kv.keys_matching(LOT_PREFIX, later).unwrap().is_empty(),
        "a listagem podou a vencida"
    );
}
