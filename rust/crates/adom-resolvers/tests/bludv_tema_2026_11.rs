//! Tema de 2026-11 do BLUDV (bludv2.com). `bludvfilmes.xyz` e
//! `bludvfilmes2.xyz` passaram a responder 301 → bludv2.com, e a busca de lá
//! não tem mais `<div class="post">`: os resultados vêm em
//! `<article class="post …"><h2 class="post-titulo"><a href>`. Sem a forma
//! nova o resolver devolvia 0 posts com o site no ar.
//!
//! Fixture: página real de `https://bludv2.com/?s=coringa`, capturada em
//! 2026-11-26.

use adom_resolvers::sites::bludv::{parse_search_entries, parse_search_posts};

const CORINGA: &str = include_str!("fixtures/bludv2_busca_coringa.html");
const BASE: &str = "https://bludv2.com/?s=coringa";

#[test]
fn busca_do_tema_novo_le_os_artigos_na_ordem() {
    let posts = parse_search_posts(CORINGA, BASE);
    assert_eq!(posts.len(), 7, "a página real tem 7 resultados");
    assert_eq!(
        posts[0].url,
        "https://bludv2.com/coringa-delirio-a-dois-2024-29-10-2024/"
    );
    assert_eq!(
        posts[0].title,
        "Coringa: Delírio a Dois (2024) WEB-DL 720p/1080p/4K Dual Áudio"
    );
    assert!(
        posts[1].title.starts_with("Coringa Torrent"),
        "o 2º é o Coringa de 2019: {}",
        posts[1].title
    );
}

#[test]
fn barra_lateral_de_posters_nao_vira_resultado() {
    // O "Top Filmes" da lateral (`<a class="poster">`) traz obras que não têm
    // nada a ver com a busca; só os `<article>` são resultado.
    let posts = parse_search_posts(CORINGA, BASE);
    assert!(posts.iter().all(|p| !p.url.contains("the-mosquito-bowl")));
}

#[test]
fn poster_e_titulo_original_saem_do_cartao_novo() {
    let entries = parse_search_entries(CORINGA, BASE);
    let first = &entries[0];
    assert_eq!(first.original.as_deref(), Some("Joker: Folie à Deux"));
    assert!(first
        .post
        .poster
        .as_deref()
        .is_some_and(|p| p.starts_with("https://image.tmdb.org/")));
}

#[test]
fn tema_antigo_continua_lido() {
    let html = r#"<div class="post"><div class="title"><a href="/filme-x/">Filme X (2020)</a></div></div>"#;
    let posts = parse_search_posts(html, "https://bludvfilmes2.xyz/?s=x");
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].url, "https://bludvfilmes2.xyz/filme-x/");
}
