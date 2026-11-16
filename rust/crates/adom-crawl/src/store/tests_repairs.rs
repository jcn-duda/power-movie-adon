//! Testes da camada store — parte 4: as melhorias medidas em 2026-11-15
//! (fresh-first da fila de portões, veredito terminal de portão expirado,
//! reparos únicos de boot e revisita de no-torrent antigos). Separado pela
//! catraca de 400 linhas; helpers (`temp_store`/`sample_url`) de `super::tests`.

use super::ops::{now_ms, GatePendingRecord};
use super::tests::{sample_url, temp_store};
use super::*;

// ─── helpers locais ──────────────────────────────────────────────────────────

/// Semeia uma linha no estado dado (upsert + UPDATE do estado/dano).
fn semear_estado(
    store: &CrawlStore,
    site: &str,
    key: &str,
    status: &str,
    releases: i64,
    error: &str,
    next_at: i64,
) {
    let mut row = sample_url(key);
    row.site = site.to_string();
    row.lastmod = "2026-10-01T00:00:00Z".to_string();
    row.imdb = Some("tt0000001".to_string());
    store.upsert_urls(site, &[row]).expect("upsert");
    store
        .conn()
        .execute(
            "UPDATE crawl_url SET status = ?3, releases = ?4, error = ?5, next_at = ?6, \
             tries = 2, progress = '{\"v\":1}' WHERE site = ?1 AND url_key = ?2",
            rusqlite::params![site, key, status, releases, error, next_at],
        )
        .expect("semeia estado");
}

/// Semeia um post com pendência de portão (a marcação do motor).
fn semear_portao(store: &CrawlStore, site: &str, key: &str) {
    let mut row = sample_url(key);
    row.site = site.to_string();
    store.upsert_urls(site, &[row]).expect("upsert");
    store
        .mark_gate_pending(
            site,
            key,
            &GatePendingRecord {
                gate: format!("https://bludvfilmes1.xyz/gate/go{key}"),
                title: "Filme 2024 1080p Dual".to_string(),
                year: 2024,
                imdb: "tt1111111".to_string(),
                kind: "movie".to_string(),
            },
        )
        .expect("portão");
}

/// Lê `(status, tries, next_at, error, progress, lastmod)` da linha.
fn ler(store: &CrawlStore, site: &str, key: &str) -> (String, u32, i64, String, String, String) {
    store
        .conn()
        .query_row(
            "SELECT status, tries, next_at, error, progress, lastmod FROM crawl_url \
             WHERE site = ?1 AND url_key = ?2",
            [site, key],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .expect("linha")
}

fn portoes(store: &CrawlStore) -> i64 {
    store
        .conn()
        .query_row("SELECT COUNT(*) FROM crawl_gate", [], |r| r.get(0))
        .expect("contagem de portões")
}

// ─── fresh-first da fila de portões (P1) ─────────────────────────────────────

/// O lote serve o portão MAIS NOVO primeiro (`added_at` DESC): o TTL medido é
/// ~26–31 dias, então o mais novo tem a maior chance de vivo. A ordem antiga
/// (`checked_at` ASC) drenava a fila de expirados na frente dos novos.
#[test]
fn take_gate_batch_serve_o_portao_mais_novo_primeiro() {
    let (store, _dir) = temp_store();
    semear_portao(&store, "bludv-cardigann", "/velho/");
    semear_portao(&store, "bludv-cardigann", "/novo/");
    // checked_at e added_at divergem de propósito: a ordem antiga serviria
    // /velho/ (checked_at menor); o fresh-first serve /novo/.
    for (key, carimbo) in [("/velho/", 1000_i64), ("/novo/", 2000_i64)] {
        store
            .conn()
            .execute(
                "UPDATE crawl_url SET checked_at = ?1 WHERE url_key = ?2",
                rusqlite::params![carimbo, key],
            )
            .expect("checked_at");
        store
            .conn()
            .execute(
                "UPDATE crawl_gate SET added_at = ?1 WHERE url_key = ?2",
                rusqlite::params![carimbo, key],
            )
            .expect("added_at");
    }
    let lote = store.take_gate_batch(1).expect("lote");
    assert_eq!(lote.len(), 1);
    assert_eq!(lote[0].row.url_key, "/novo/", "fresh-first");
    let todos = store.take_gate_batch(10).expect("lote cheio");
    let chaves: Vec<&str> = todos.iter().map(|g| g.row.url_key.as_str()).collect();
    assert_eq!(chaves, vec!["/novo/", "/velho/"]);
}

// ─── veredito terminal de portão expirado (P1) ───────────────────────────────

/// `expire_gate` fecha a linha como `error` terminal (tries debitado), apaga o
/// portão e SÓ age em `gate-pending`: repetir numa linha já fechada é no-op.
#[test]
fn expire_gate_fecha_terminal_so_em_gate_pending() {
    let (store, _dir) = temp_store();
    semear_portao(&store, "bludv-cardigann", "/post/");
    store
        .expire_gate(
            "bludv-cardigann",
            "/post/",
            "resolver: http 502 bludv: portao de anuncio (link expirado)",
        )
        .expect("expirar");
    let (status, tries, next_at, erro, _, _) = ler(&store, "bludv-cardigann", "/post/");
    assert_eq!((status.as_str(), tries, next_at), ("error", 1, 0));
    assert!(erro.contains("expirado"), "veredito preservado");
    assert_eq!(portoes(&store), 0, "portão saiu da fila");
    // Linha já fechada: a segunda chamada não debita nem apaga nada.
    store
        .expire_gate("bludv-cardigann", "/post/", "de novo")
        .expect("repete");
    assert_eq!(ler(&store, "bludv-cardigann", "/post/").1, 1);
}

/// O reparo único fecha os portões expirados PERSISTENTES (tries>=10 com
/// veredito "expirado") e só eles: jovens e sem o veredito seguem na fila.
#[test]
fn reparo_gate_expirado_fecha_os_persistentes_uma_vez() {
    let (store, _dir) = temp_store();
    let site = "bludv-cardigann";
    let expirado = "resolver: http 502 bludv: portao de anuncio (link expirado)";
    for (key, tries, erro) in [
        ("/velho-expirado/", 12_u32, expirado),
        ("/jovem-expirado/", 2, expirado),
        (
            "/velho-outro/",
            15,
            "resolver: http 502 bludv: portao de anuncio (x)",
        ),
    ] {
        semear_portao(&store, site, key);
        store
            .conn()
            .execute(
                "UPDATE crawl_url SET tries = ?1, error = ?2 WHERE site = ?3 AND url_key = ?4",
                rusqlite::params![tries, erro, site, key],
            )
            .expect("dano");
    }
    semear_estado(&store, site, "/pago/", "done", 3, "", 0);

    assert_eq!(store.repair_gate_expired_once().expect("reparo"), 1);
    let (status, _, next_at, erro, _, _) = ler(&store, site, "/velho-expirado/");
    assert_eq!((status.as_str(), next_at), ("error", 0));
    assert!(erro.contains("expirado"), "veredito fica para diagnóstico");
    assert_eq!(portoes(&store), 2, "só o portão alvo saiu da fila");
    assert_eq!(ler(&store, site, "/jovem-expirado/").0, "gate-pending");
    assert_eq!(ler(&store, site, "/velho-outro/").0, "gate-pending");
    assert_eq!(ler(&store, site, "/pago/").0, "done");
    // Idempotente: o marcador global fecha a 2ª rodada.
    assert_eq!(store.repair_gate_expired_once().expect("repete"), 0);
}

// ─── reparo único das linhas presas legadas (P2+P3) ──────────────────────────

/// O reparo devolve à fila os fantasmas `done/releases=0` e os erros terminais
/// RETENTÁVEIS da whitelist medida — e SÓ eles: estruturais (`series_truncated`,
/// `blocked_host`), erros em recuo (`next_at>0`) e acervo real não se tocam.
#[test]
fn reparo_stuck_legacy_revive_fantasmas_e_erros_retentaveis() {
    let (store, _dir) = temp_store();
    let site = "redetorrent-cardigann";
    let futuro = now_ms() + 60_000;
    for (key, status, releases, erro, next_at) in [
        ("/fantasma/", "done", 0_i64, "", 0_i64),
        ("/acervo/", "done", 4, "", 0),
        ("/sem-nomes/", "error", 0, "ingest: catalogo-sem-nomes", 0),
        ("/404/", "error", 0, "http_404", 0),
        ("/truncada/", "error", 0, "series_truncated: teto", 0),
        ("/blocked/", "error", 0, "blocked_host: evil.site", 0),
        ("/recuando/", "error", 0, "http_500", futuro),
        ("/sem-torrent/", "no-torrent", 0, "", 0),
    ] {
        semear_estado(&store, site, key, status, releases, erro, next_at);
    }

    assert_eq!(
        store.repair_stuck_legacy_once().expect("reparo"),
        3,
        "fantasma + sem-nomes + 404"
    );
    for key in ["/fantasma/", "/sem-nomes/", "/404/"] {
        let (status, tries, next_at, erro, progress, lastmod) = ler(&store, site, key);
        assert_eq!(
            (status.as_str(), tries, next_at),
            ("pending", 0, 0),
            "{key}"
        );
        assert_eq!((erro.as_str(), progress.as_str()), ("", ""), "{key}");
        assert_eq!(
            lastmod, "2026-10-01T00:00:00Z",
            "identidade preservada: {key}"
        );
    }
    // Não-alvos intactos.
    assert_eq!(ler(&store, site, "/acervo/").0, "done");
    assert_eq!(ler(&store, site, "/truncada/").0, "error");
    assert_eq!(ler(&store, site, "/blocked/").0, "error");
    assert_eq!(ler(&store, site, "/recuando/").0, "error");
    assert_eq!(ler(&store, site, "/sem-torrent/").0, "no-torrent");
    // Idempotente: o marcador fecha a 2ª rodada, mesmo com dano "novo".
    semear_estado(&store, site, "/fantasma-2/", "done", 0, "", 0);
    assert_eq!(store.repair_stuck_legacy_once().expect("repete"), 0);
    assert_eq!(ler(&store, site, "/fantasma-2/").0, "done");
}

// ─── revisita de no-torrent antigos (P5) ─────────────────────────────────────

/// A revisita devolve as MAIS VELHAS além do corte, no limite pedido,
/// preservando a identidade; recentes, nunca lidas e acervo não se tocam.
#[test]
fn revisita_devolve_as_no_torrent_mais_velhas_preservando_identidade() {
    let (store, _dir) = temp_store();
    let agora = now_ms();
    let dia: i64 = 24 * 3_600_000;
    for (key, idade) in [("/40d/", 40_i64), ("/30d/", 30), ("/20d/", 20), ("/5d/", 5)] {
        semear_estado(&store, "site-a", key, "no-torrent", 0, "", 0);
        store
            .conn()
            .execute(
                "UPDATE crawl_url SET checked_at = ?1 WHERE site = 'site-a' AND url_key = ?2",
                rusqlite::params![agora - idade * dia, key],
            )
            .expect("carimbo");
    }
    // Nunca lida (checked_at=0), acervo velho e outro site: fora da revisita.
    semear_estado(&store, "site-a", "/nunca/", "no-torrent", 0, "", 0);
    semear_estado(&store, "site-a", "/acervo/", "done", 3, "", 0);
    semear_estado(&store, "site-b", "/velha-b/", "no-torrent", 0, "", 0);
    for (site, key) in [("site-a", "/acervo/"), ("site-b", "/velha-b/")] {
        store
            .conn()
            .execute(
                "UPDATE crawl_url SET checked_at = ?1 WHERE site = ?2 AND url_key = ?3",
                rusqlite::params![agora - 50 * dia, site, key],
            )
            .expect("carimbo");
    }

    let corte = agora - 15 * dia;
    assert_eq!(
        store
            .revisit_stale_no_torrent("site-a", corte, 2)
            .expect("revisita"),
        2,
        "o limite escolhe as duas mais velhas"
    );
    assert_eq!(ler(&store, "site-a", "/40d/").0, "pending");
    assert_eq!(ler(&store, "site-a", "/30d/").0, "pending");
    assert_eq!(
        ler(&store, "site-a", "/20d/").0,
        "no-torrent",
        "ficou para a próxima"
    );
    assert_eq!(
        ler(&store, "site-a", "/40d/").5,
        "2026-10-01T00:00:00Z",
        "lastmod preservado"
    );
    // Próxima rodada: a que sobrou além do corte volta; o resto não se toca.
    assert_eq!(
        store
            .revisit_stale_no_torrent("site-a", corte, 3)
            .expect("revisita"),
        1
    );
    assert_eq!(ler(&store, "site-a", "/20d/").0, "pending");
    assert_eq!(ler(&store, "site-a", "/5d/").0, "no-torrent", "recente");
    assert_eq!(
        ler(&store, "site-a", "/nunca/").0,
        "no-torrent",
        "nunca lida"
    );
    assert_eq!(ler(&store, "site-a", "/acervo/").0, "done");
    assert_eq!(
        ler(&store, "site-b", "/velha-b/").0,
        "no-torrent",
        "outro site"
    );
    // Esgotado o alvo, a revisita é no-op (não re-casa linha já pending).
    assert_eq!(
        store
            .revisit_stale_no_torrent("site-a", corte, 3)
            .expect("repete"),
        0
    );
}
