//! Testes da camada store (CrawlStore) — parte 3: as regras novas da fila
//! (Fases 0.4 e 1.1) e a preservação da sonda no "Zerar site".
//!
//! Cobre: `skip_sites` sem gastar vaga, ordem priorizada por IMDb da semente,
//! adiamento (Retry-After 429/503) sem debitar `tries`, reparo único das tries
//! queimadas por 429 e `clear_site` preservando o veredito da sonda. Os helpers
//! (`temp_store` / `sample_url`) vêm de `super::tests`, como em `tests_part2`.

use super::ops::PROBE_VERDICT_KEY;
use super::tests::{sample_url, temp_store};
use super::*;

// ─── helpers locais ──────────────────────────────────────────────────────────

/// Marca a próxima URL due do site com o resultado dado (a fila serve a menor
/// `url_key` primeiro quando os `next_at` empatarem em 0).
fn marcar(store: &CrawlStore, site: &str, result: &MarkResult) {
    let row = store
        .take_next(site)
        .expect("take")
        .expect("URL due para marcar");
    store.mark_result(site, &row.url_key, result).expect("mark");
}

/// Lê `(tries, status, error)` da linha — a prova direta do que o UPDATE fez.
fn linha(store: &CrawlStore, site: &str, key: &str) -> (u32, String, String) {
    store
        .conn()
        .query_row(
            "SELECT tries, status, error FROM crawl_url WHERE site = ?1 AND url_key = ?2",
            [site, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("linha")
}

// ─── skip_sites (contrato motor↔store) ──────────────────────────────────────

/// Site em espera sai do recorte SEM gastar vaga do lote: a vaga é preenchida
/// por quem pode trabalhar, e o site parado continua due e intacto.
#[test]
fn take_next_batch_ignora_site_em_espera_sem_gastar_vaga() {
    let (store, _dir) = temp_store();
    for site in ["site-a", "site-b", "site-c"] {
        store
            .upsert_urls(site, &[sample_url("/1")])
            .expect("upsert");
    }

    let skip = vec!["site-a".to_string()];
    let batch = store.take_next_batch(2, &skip).expect("batch");

    let sites: Vec<&str> = batch.iter().map(|r| r.site.as_str()).collect();
    assert_eq!(
        sites,
        ["site-b", "site-c"],
        "site-a saiu do recorte e a vaga dele foi preenchida"
    );
    assert!(store.has_due("site-a").expect("has_due"), "continua due");
    let c = store.counters("site-a").expect("counters");
    assert_eq!(c.pending, 1, "nada do site em espera foi reivindicado");
    assert_eq!(c.inflight, 0);
}

// ─── ordem priorizada (Fase 1.1) ─────────────────────────────────────────────

/// Dentro do site, a URL com IMDb da semente sai primeiro, mesmo com `next_at`
/// maior — a novidade já identificada rende busca na hora. (Limite: a coorte
/// `seed:v1:cohort` não é acessível do sidecar sem rede; a prioridade local por
/// imdb é o porte possível.)
#[test]
fn take_next_batch_prioriza_url_com_imdb_da_semente() {
    let (store, _dir) = temp_store();
    let sem_imdb = sample_url("/sem-imdb");
    let mut com_imdb = sample_url("/com-imdb");
    com_imdb.imdb = Some("tt0000001".to_string());
    com_imdb.next_at = 500; // mais nova que a sem imdb (next_at = 0)
    store
        .upsert_urls("site-a", &[sem_imdb, com_imdb])
        .expect("upsert");

    let batch = store.take_next_batch(1, &[]).expect("batch");
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].url_key, "/com-imdb",
        "imdb da semente vence o next_at"
    );
}

// ─── adiamento não gasta tries (contrato motor↔store) ───────────────────────

/// `next_at > 0` com error `http_429`/`http_503` é Retry-After: a página não
/// foi trabalhada, então `tries` NÃO avança e o status volta a `pending`. Erro
/// real com backoff debita 1; 429 sem `next_at` fecha terminal como o motor
/// pediu (não é adiamento).
#[test]
fn adiamento_http429_http503_nao_gasta_tries() {
    let (store, _dir) = temp_store();
    store
        .upsert_urls(
            "test-site",
            &[
                sample_url("/a"),
                sample_url("/b"),
                sample_url("/c"),
                sample_url("/d"),
            ],
        )
        .expect("upsert");

    // Ordem das marcas: a fila serve o menor `url_key` (next_at empata em 0),
    // então /a, /b e /c recebem os resultados abaixo NESTA ordem.
    for (error, next_at) in [
        ("http_429:retry-after-30", 60_000),
        ("http_503:retry-after-5", 5_000),
        ("timeout:leitura", 60_000),
    ] {
        marcar(
            &store,
            "test-site",
            &MarkResult {
                status: "error".to_string(),
                imdb: None,
                title: None,
                year: None,
                releases: 0,
                error: error.to_string(),
                progress: String::new(),
                next_at,
                request_cost: 1,
            },
        );
    }
    marcar(
        &store,
        "test-site",
        &MarkResult {
            status: "error".to_string(),
            imdb: None,
            title: None,
            year: None,
            releases: 0,
            error: "http_429:sem-adiamento".to_string(),
            progress: String::new(),
            next_at: 0,
            request_cost: 1,
        },
    );

    let (tries_a, status_a, error_a) = linha(&store, "test-site", "/a");
    assert_eq!(tries_a, 0, "429 adiando não debita tries");
    assert_eq!(status_a, "pending");
    assert_eq!(error_a, "http_429:retry-after-30");
    let (tries_b, _, _) = linha(&store, "test-site", "/b");
    assert_eq!(tries_b, 0, "503 adiando não debita tries");
    let (tries_c, _, _) = linha(&store, "test-site", "/c");
    assert_eq!(tries_c, 1, "erro real com backoff debita");
    let (tries_d, status_d, _) = linha(&store, "test-site", "/d");
    assert_eq!(tries_d, 1, "429 sem next_at não é adiamento");
    assert_eq!(status_d, "error", "fecha terminal como o motor pediu");
}

// ─── reparo único das tries de adiamento (Fase 0.4) ─────────────────────────

/// O reparo zera tries=1 das linhas com error `http_429*` em qualquer site (o
/// 429 é do Cloudflare, não de um site), preserva o resto da linha e não
/// repete: o marcador global fecha a 2ª rodada.
#[test]
fn reparo_http429_zera_tries_uma_vez_e_nao_toca_o_resto() {
    let (store, _dir) = temp_store();
    for (site, key, tries, error) in [
        ("vacatorrent", "/a", 1, "http_429:retry-after-30"),
        ("vacatorrent", "/b", 2, "http_429:retry-after-30"),
        ("vacatorrent", "/c", 1, "timeout:leitura"),
        ("nerdfilmes", "/d", 1, "http_429:retry-after-30"),
        ("nerdfilmes", "/e", 1, "http_503:retry-after-5"),
    ] {
        store.upsert_urls(site, &[sample_url(key)]).expect("upsert");
        store
            .conn()
            .execute(
                "UPDATE crawl_url SET tries = ?1, error = ?2, next_at = 60000 \
                 WHERE site = ?3 AND url_key = ?4",
                rusqlite::params![tries, error, site, key],
            )
            .expect("semeia dano");
    }

    assert_eq!(
        store.repair_http429_once().expect("reparo"),
        2,
        "/a e /d são o dano conhecido"
    );

    // Zeradas, com o RESTO da linha intacto (erro e next_at preservados).
    assert_eq!(linha(&store, "vacatorrent", "/a").0, 0);
    assert_eq!(
        linha(&store, "vacatorrent", "/a").2,
        "http_429:retry-after-30"
    );
    assert_eq!(linha(&store, "nerdfilmes", "/d").0, 0);
    // Não-alvo intacto.
    assert_eq!(
        linha(&store, "vacatorrent", "/b").0,
        2,
        "só tries=1 é o dano conhecido"
    );
    assert_eq!(
        linha(&store, "vacatorrent", "/c").0,
        1,
        "erro que não é 429 não se toca"
    );
    assert_eq!(
        linha(&store, "nerdfilmes", "/e").0,
        1,
        "503 não é o dano deste reparo"
    );

    // Idempotente: a marca `repair:http429-tries-v1` fecha a 2ª rodada.
    assert_eq!(store.repair_http429_once().expect("repete"), 0);
}

// ─── reparo único das filas presas do Mico ──────────────────────────────────

/// O reparo devolve à fila os dois danos conhecidos do Mico, UMA vez:
///
/// - fantasma `done/releases=0` (conclusão falsa de passes vazios) → reset
///   COMPLETO: a obra é relida do zero e o veredito honesto sai do motor atual;
/// - `partial` LEGADO (o motor atual grava `pending` + `next_at`, nunca
///   `partial` persistente) → volta à fila PRESERVANDO o progresso: a série
///   retoma de onde parou em vez de re-sondar tudo.
///
/// `done` com acervo, `no-torrent` e outros sites não se tocam.
#[test]
fn reparo_mico_devolve_fantasmas_e_parciais_legados_uma_vez() {
    let (store, _dir) = temp_store();
    let prog = r#"{"v":1,"doneCards":["1:1"],"totalCards":2}"#;
    for (site, key, status, releases, progress) in [
        ("mico", "/crawl/series/tt0000001/", "done", 0, prog), // fantasma
        ("mico", "/crawl/series/tt0000002/", "partial", 3, prog), // legado
        ("mico", "/crawl/movie/tt0000003/", "done", 5, ""),    // acervo real
        ("mico", "/crawl/movie/tt0000004/", "no-torrent", 0, ""),
        ("nerdfilmes", "/filme/x/", "done", 0, prog), // outro site
    ] {
        store.upsert_urls(site, &[sample_url(key)]).expect("upsert");
        store
            .conn()
            .execute(
                "UPDATE crawl_url SET status = ?1, releases = ?2, progress = ?3, \
                 tries = 2, next_at = 9, error = 'series_truncated: teto' \
                 WHERE site = ?4 AND url_key = ?5",
                rusqlite::params![status, releases, progress, site, key],
            )
            .expect("semeia dano");
    }

    assert_eq!(store.repair_mico_stuck_once().expect("reparo"), 2);

    let ler = |key: &str| -> (String, i64, i64, String, String) {
        store
            .conn()
            .query_row(
                "SELECT status, tries, next_at, progress, error FROM crawl_url \
                 WHERE site = 'mico' AND url_key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("linha")
    };
    // Fantasma: reset completo — relê do zero, sem progresso velho.
    let (status, tries, next_at, progress, error) = ler("/crawl/series/tt0000001/");
    assert_eq!((status.as_str(), tries, next_at), ("pending", 0, 0));
    assert_eq!(progress, "", "fantasma é relido do zero");
    assert_eq!(error, "");
    // Partial legado: retoma — progresso e acervo acumulado sobrevivem.
    let (status, tries, next_at, progress, _) = ler("/crawl/series/tt0000002/");
    assert_eq!((status.as_str(), tries, next_at), ("pending", 0, 0));
    assert_eq!(progress, prog, "a retomada continua de onde parou");
    let releases: i64 = store
        .conn()
        .query_row(
            "SELECT releases FROM crawl_url WHERE site = 'mico' AND url_key = ?1",
            ["/crawl/series/tt0000002/"],
            |r| r.get(0),
        )
        .expect("releases");
    assert_eq!(releases, 3, "acervo do partial não se perde");
    // Não-alvos intactos.
    assert_eq!(ler("/crawl/movie/tt0000003/").0, "done");
    assert_eq!(ler("/crawl/movie/tt0000004/").0, "no-torrent");
    assert_eq!(
        linha(&store, "nerdfilmes", "/filme/x/").1,
        "done",
        "o reparo é só do Mico"
    );

    // Idempotente: o marcador fecha a 2ª rodada, mesmo com dano "novo".
    store
        .conn()
        .execute(
            "UPDATE crawl_url SET status = 'done', releases = 0 \
             WHERE site = 'mico' AND url_key = '/crawl/movie/tt0000004/'",
            [],
        )
        .expect("refecha");
    assert_eq!(store.repair_mico_stuck_once().expect("repete"), 0);
    assert_eq!(ler("/crawl/movie/tt0000004/").0, "done");
}

// ─── zerar site preserva a sonda (Fase 3.5) ──────────────────────────────────

/// "Zerar site" apaga a fila e o estado, mas mantém o veredito da sonda: o
/// gate sem veredito barra um site já autorizado — zerar a fila não pode
/// desautorizar o que foi medido (o mesmo que o TS fazia no `resetSite`).
#[test]
fn clear_site_preserva_veredito_da_sonda() {
    let (store, _dir) = temp_store();
    store
        .upsert_urls("site-a", &[sample_url("/1")])
        .expect("upsert");
    store
        .set_state(
            "site-a",
            PROBE_VERDICT_KEY,
            "{\"site\":\"site-a\",\"verdict\":\"go\"}",
        )
        .expect("veredito da sonda");
    store
        .set_state("site-a", "cursor:movie", "pagina-42")
        .expect("cursor");
    store
        .upsert_urls("site-b", &[sample_url("/1")])
        .expect("upsert b");

    store.clear_site("site-a").expect("clear");

    let verdict = store.get_state("site-a", PROBE_VERDICT_KEY).expect("get");
    assert_eq!(
        verdict.as_deref(),
        Some("{\"site\":\"site-a\",\"verdict\":\"go\"}"),
        "o gate continua com o que a sonda mediu"
    );
    assert!(
        store
            .get_state("site-a", "cursor:movie")
            .expect("cursor")
            .is_none(),
        "o resto do estado é apagado"
    );
    assert_eq!(store.counters("site-a").expect("counters").pending, 0);
    assert_eq!(
        store.counters("site-b").expect("outro site").pending,
        1,
        "outro site intacto"
    );
}
