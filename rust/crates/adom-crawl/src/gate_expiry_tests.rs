//! Veredito terminal do portão EXPIRADO persistente (medido em 2026-11-15:
//! TTL ~26–31 dias, portões do host antigo respondem "Referência expirada").
//! Separado do `gate_tests.rs` pela catraca de 400 linhas; harness em
//! `gate/support.rs`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::support::*;
use super::*;
use crate::store::ops::CrawlStore;

/// Resolver que devolve o veredito REAL de link morto (o corpo que o resolver
/// do bludv manda quando a referência do portão expirou).
struct Expirado;

impl GateResolver for Expirado {
    fn resolve<'a>(&'a self, _gate: &'a str, _post: &'a str) -> GateFuture<'a> {
        Box::pin(async {
            Err("resolver: http 502 bludv: portao de anuncio (link expirado)".to_string())
        })
    }
}

/// Semeia o portão e força o contador de tentativas (o teste não espera os
/// recuos entre rodadas).
fn semear_com_tries(caminho: &str, key: &str, tries: u32) {
    semear_gate(caminho, key, GATE);
    let db = CrawlStore::open(caminho).expect("abrir para semear tries");
    db.conn()
        .execute(
            "UPDATE crawl_url SET tries = ?1 WHERE url_key = ?2",
            rusqlite::params![tries, key],
        )
        .expect("semear tries");
}

/// Conta os portões restantes na `crawl_gate` (independe do `next_at`).
fn portoes_restantes(caminho: &str) -> i64 {
    let db = CrawlStore::open(caminho).expect("reabrir");
    db.conn()
        .query_row("SELECT COUNT(*) FROM crawl_gate", [], |r| r.get(0))
        .expect("contagem")
}

/// 3ª falha com veredito "expirado" fecha a linha como `error` terminal e tira
/// o portão da fila — pagar de novo um link morto é queimar FlareSolverr e o
/// teto por hora na frente dos portões novos.
#[tokio::test]
async fn portao_expirado_persistente_vira_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let caminho = dir.path().join("crawl.db").to_str().unwrap().to_string();
    semear_com_tries(&caminho, "post-1", 2); // a próxima falha é a 3ª
    let ingest = IngestFake::novo(Recusa::Nunca);
    let node = node_falso(false).await;
    let ex = executor(
        &caminho,
        &node,
        plug(&ingest),
        Arc::new(Expirado),
        Arc::new(FlareFake { ocupado: false }),
        4,
    );
    let report = ex.run_round().await.expect("rodada");
    assert_eq!(report.paid, 0);
    let (status, tries, next_at) = ler_linha(&caminho, "post-1");
    assert_eq!(status, "error", "portão expirado persistente é terminal");
    assert_eq!(tries, 3, "a tentativa que trouxe o veredito é debitada");
    assert_eq!(next_at, 0, "terminal: sem próxima tentativa agendada");
    assert_eq!(portoes_restantes(&caminho), 0, "o portão saiu da fila");
    // O veredito fica gravado (diagnóstico e casamento do reparo de boot).
    let db = CrawlStore::open(&caminho).expect("reabrir");
    let erro: String = db
        .conn()
        .query_row(
            "SELECT error FROM crawl_url WHERE url_key = 'post-1'",
            [],
            |r| r.get(0),
        )
        .expect("erro");
    assert!(erro.contains("expirado"), "veredito preservado: {erro}");
}

/// Expirado JOVEM (< 3 tentativas) ainda recua como qualquer falha: o veredito
/// pode ser transitório (cadeia do protetor em obras) antes de virar certeza.
#[tokio::test]
async fn expirado_jovem_ainda_recua() {
    let dir = tempfile::tempdir().expect("tempdir");
    let caminho = dir.path().join("crawl.db").to_str().unwrap().to_string();
    semear_gate(&caminho, "post-1", GATE); // tries = 0
    let ingest = IngestFake::novo(Recusa::Nunca);
    let node = node_falso(false).await;
    let ex = executor(
        &caminho,
        &node,
        plug(&ingest),
        Arc::new(Expirado),
        Arc::new(FlareFake { ocupado: false }),
        4,
    );
    ex.run_round().await.expect("rodada");
    let (status, tries, next_at) = ler_linha(&caminho, "post-1");
    assert_eq!(status, "gate-pending", "jovem demais para o terminal");
    assert_eq!(tries, 1);
    assert!(next_at > now_ms(), "espera o recuo");
    assert_eq!(portoes_restantes(&caminho), 1, "o portão segue na fila");
}

/// "pendente" da cadeia em curso NUNCA vira terminal, mesmo com tries alto:
/// não é falha, é a cadeia do resolver ainda trabalhando.
#[tokio::test]
async fn pendente_com_tries_alto_nao_vira_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let caminho = dir.path().join("crawl.db").to_str().unwrap().to_string();
    semear_com_tries(&caminho, "post-1", 15);
    let ingest = IngestFake::novo(Recusa::Nunca);
    let node = node_falso(false).await;
    let chamadas = Arc::new(AtomicUsize::new(0));
    let ex = executor(
        &caminho,
        &node,
        plug(&ingest),
        falso(chamadas.clone(), true), // Err("pendente no resolver")
        Arc::new(FlareFake { ocupado: false }),
        4,
    );
    ex.run_round().await.expect("rodada");
    assert_eq!(chamadas.load(Ordering::SeqCst), 1);
    let (status, tries, next_at) = ler_linha(&caminho, "post-1");
    assert_eq!(status, "gate-pending");
    assert_eq!(tries, 15, "pendente não debita tentativa");
    assert_eq!(next_at, 0, "pendente repete na rodada seguinte, sem recuo");
    assert_eq!(portoes_restantes(&caminho), 1);
}
