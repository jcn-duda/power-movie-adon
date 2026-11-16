//! Dreno e memo da conta: memo fresco não paga HTTP, fila vazia encerra ANTES
//! da sonda e erro do serviço segue fail-open (nunca vira memo).

use adom_autofetch::v3::{QueueCandidate, QueueV3};
use adom_index::NativeKv;
use adom_sources::now_ms;
use serde_json::json;

use super::super::enqueue::account::probe;
use super::super::enqueue::Holds;
use super::super::fakes::{FakeDebrid, SCOPE};
use super::super::Policy;
use super::{drain_one, DrainReport};

const SEARCH: &str = "tt1234567:1:2";
const ADAPTER: &str = "alldebrid";
const ACCOUNT: &str = "conta-de-teste";
const ACCOUNT_PREFIX: &str = "autofetch-worker:v1:account:";
/// Janela pinada no cfg abaixo (`autoFetchPauseRefreshMs`).
const REFRESH: i64 = 60_000;

fn hash_of(n: u32) -> String {
    format!("{n:040x}")
}

fn kv() -> NativeKv {
    let kv = NativeKv::open_memory().unwrap();
    // Config PINADA: os testes de env escrevem no processo em paralelo.
    kv.put_json(
        "cfg:v1:autofetch",
        &json!({
            "autoFetchQueue": true,
            "autoFetchQueueDepth": 6,
            "autoFetchPauseAt": 2000,
            "autoFetchPauseRefreshMs": REFRESH
        }),
        600,
        now_ms(),
    )
    .unwrap();
    kv
}

fn serving(used: Option<u64>) -> FakeDebrid {
    FakeDebrid {
        known: true,
        accept: true,
        account_used: used,
        ..FakeDebrid::default()
    }
}

/// Um candidato BR na fila da busca (o que o enqueue deixaria para o dreno).
fn queued(kv: &NativeKv, hash: &str, now: i64) {
    let candidate = QueueCandidate {
        info_hash: hash.to_string(),
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

fn queued_hashes(kv: &NativeKv, now: i64) -> Vec<String> {
    QueueV3::new(kv)
        .read_queue(SEARCH, now)
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.info_hash)
        .collect()
}

async fn drain(
    kv: &NativeKv,
    debrid: &FakeDebrid,
    paused: bool,
    search: &str,
    now: i64,
) -> DrainReport {
    drain_one(
        kv,
        Policy::fixture(),
        &Holds::default(),
        paused,
        debrid,
        SCOPE,
        search,
        now,
    )
    .await
    .unwrap()
}

/// (a) O enqueue já perguntou dentro da janela: o dreno lê o MESMO memo do KV e
/// não repete o HTTP — e o memo continua valendo para o próximo enqueue.
#[tokio::test]
async fn memo_fresco_nao_paga_http_no_dreno() {
    let kv = kv();
    let now = now_ms();
    let debrid = serving(Some(10));
    assert_eq!(
        probe(&kv, ADAPTER, ACCOUNT, &debrid, REFRESH, now).await,
        Some(10)
    );
    queued(&kv, &hash_of(1), now);

    let report = drain(&kv, &debrid, false, SEARCH, now).await;
    assert_eq!(report.started, Some(hash_of(1)));
    assert_eq!(debrid.account_calls(), 1, "memo fresco: o dreno não repete");

    // A janela é compartilhada: o enqueue seguinte também não paga.
    probe(&kv, ADAPTER, ACCOUNT, &debrid, REFRESH, now).await;
    assert_eq!(debrid.account_calls(), 1);
}

/// (a2) Memo fresco com uso NO teto: bloqueia pelo fato medido, sem HTTP novo e
/// sem condenar a cabeça da fila.
#[tokio::test]
async fn memo_fresco_no_teto_bloqueia_sem_http() {
    let kv = kv();
    let now = now_ms();
    let debrid = serving(Some(2_500));
    probe(&kv, ADAPTER, ACCOUNT, &debrid, REFRESH, now).await;
    queued(&kv, &hash_of(1), now);

    let report = drain(&kv, &debrid, false, SEARCH, now).await;
    assert_eq!(report.blocked, Some("account-gate"));
    assert_eq!(report.remaining, 1);
    assert!(report.started.is_none());
    assert_eq!(debrid.account_calls(), 1, "o veredito veio do memo");
    assert_eq!(queued_hashes(&kv, now), vec![hash_of(1)]);
}

/// (b) Fila vazia encerra ANTES da sonda de conta, com o MESMO report que o
/// `drain_next` produziria (ordem dos portões: disabled → paused → empty).
#[tokio::test]
async fn fila_vazia_termina_antes_da_sonda_de_conta() {
    let kv = kv();
    let now = now_ms();
    let debrid = serving(Some(10));

    let report = drain(&kv, &debrid, false, SEARCH, now).await;
    assert_eq!(report.blocked, Some("empty"));
    assert_eq!(report.remaining, 0);

    let report = drain(&kv, &debrid, true, SEARCH, now).await;
    assert_eq!(report.blocked, Some("paused"), "pausa vem antes de empty");

    let report = drain(&kv, &debrid, false, "", now).await;
    assert_eq!(report.blocked, Some("drain-disabled"));

    assert_eq!(
        debrid.account_calls(),
        0,
        "fila vazia não paga HTTP de conta"
    );
}

/// (c) Erro do serviço segue FAIL-OPEN: sem medição o gate não bloqueia, o
/// candidato sobe e o erro NUNCA vira memo.
#[tokio::test]
async fn erro_do_servico_segue_fail_open_sem_virar_memo() {
    let kv = kv();
    let now = now_ms();
    let debrid = FakeDebrid {
        known: true,
        accept: true,
        fail_account: true,
        ..FakeDebrid::default()
    };
    queued(&kv, &hash_of(1), now);

    let report = drain(&kv, &debrid, false, SEARCH, now).await;
    assert_eq!(
        report.started,
        Some(hash_of(1)),
        "sem medição = desbloqueado"
    );
    assert_eq!(debrid.account_calls(), 1);
    assert!(
        kv.keys_matching(ACCOUNT_PREFIX, now).unwrap().is_empty(),
        "erro não é cacheado: a próxima leva pergunta de novo"
    );
}
