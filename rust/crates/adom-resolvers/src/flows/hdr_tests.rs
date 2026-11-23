//! Busca do HDR pelo `index.php?busca=`: fixtures são HTML REAL salvo do site
//! em 2026-11-23 (`index.php?busca=coringa` e `?busca=fallout`).

use super::{site_queries, Selection};
use crate::profile::ResolverProfile;
use crate::sites::hdrtorrents::parse_listing_html;

const BASE: &str = "https://hdrtorrents.net";
const CORINGA: &str = include_str!("../../tests/fixtures/hdrtorrents_busca_coringa.html");
const FALLOUT: &str = include_str!("../../tests/fixtures/hdrtorrents_busca_fallout.html");

/// O que a busca do HDR entrega para uma query, pelo mesmo filtro do flow.
fn selected(html: &str, query: &str) -> Vec<String> {
    let works = parse_listing_html(html, BASE).expect("cards da busca");
    Selection::of("hdrtorrents", query)
        .apply(works)
        .into_iter()
        .map(|p| p.title)
        .collect()
}

#[test]
fn query_do_site_vai_so_com_o_nome() {
    assert_eq!(site_queries("Coringa 2019"), vec!["Coringa"]);
    assert_eq!(site_queries("Fallout S01"), vec!["Fallout"]);
    assert_eq!(site_queries("Fallout S01E03"), vec!["Fallout"]);
    assert_eq!(site_queries("Duna: Parte 2"), vec!["Duna Parte 2"]);
    // Acento fica: o site casa "Extermínio" igual a "Exterminio".
    assert_eq!(site_queries("Extermínio"), vec!["Extermínio"]);
    // "1917" é ano e nome ao mesmo tempo: sem nome sobrando, o ano fica.
    assert_eq!(site_queries("1917 2019"), vec!["1917 2019"]);
    assert!(site_queries("S01").is_empty());
}

#[test]
fn artigo_ganha_segunda_tentativa() {
    assert_eq!(site_queries("The Boys S04"), vec!["The Boys", "Boys"]);
    assert_eq!(
        site_queries("O Agente Secreto 2025"),
        vec!["O Agente Secreto", "Agente Secreto"]
    );
    // Só artigo: não sobra query enxuta, fica a original.
    assert_eq!(site_queries("The"), vec!["The"]);
}

#[test]
fn busca_coringa_traz_o_filme() {
    let works = parse_listing_html(CORINGA, BASE).expect("cards");
    assert_eq!(works.len(), 13);
    let got = selected(CORINGA, "Coringa 2019");
    assert!(
        got.iter().any(|t| t.starts_with("Coringa (2019)")),
        "{got:?}"
    );
    assert!(got.iter().all(|t| t.starts_with("Coringa")), "{got:?}");
}

/// O Jackett manda o título nu para o HDR (sem SxxEyy): as duas temporadas
/// voltam e o lixo parecido ("Efeito Fallout") fica para o filtro estrito do
/// addon, como sempre ficou.
#[test]
fn busca_fallout_traz_as_temporadas() {
    let got = selected(FALLOUT, "Fallout");
    assert!(
        got.iter().any(|t| t.contains("Fallout - 1ª Temporada")),
        "{got:?}"
    );
    assert!(
        got.iter().any(|t| t.contains("Fallout - 2ª Temporada")),
        "{got:?}"
    );
}

#[test]
fn url_de_busca_usa_o_form_do_site() {
    let hdr = ResolverProfile::new("hdrtorrents", BASE, vec![]);
    assert_eq!(
        hdr.search_url("o agente secreto").unwrap(),
        "https://hdrtorrents.net/index.php?busca=o%20agente%20secreto"
    );
}
