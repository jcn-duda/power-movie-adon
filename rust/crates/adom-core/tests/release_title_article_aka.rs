//! Título global sem o artigo inicial e com "(aka …)" antes do ano.
//!
//! Medido em 2026-11-16 (The Rejuvenator, tt0095971): "Rejuvenator 1988
//! DVDRip" e "Rejuvenatrix (aka The Rejuvenator) VHSRip 1988" eram recusados
//! como título. O primeiro porque a cobertura do nome contava o artigo; o
//! segundo porque o "aka" parecia palavra de outra obra antes do ano.

use adom_core::{filter_relevant_raw, RelevanceItem, RelevanceQuery, RelevanceSettings};

fn keeps(names: &[&str], year: &str, title: &str) -> bool {
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let query = RelevanceQuery {
        names: &names,
        year,
        alt_year: None,
        is_series: false,
        season: None,
        episode: None,
        multi_work_root: None,
        home_release_at: None,
    };
    let settings = RelevanceSettings {
        now_ms: 1_794_000_000_000,
        pre_home_release_cut: false,
        pre_home_release_margin_ms: 0,
        title_precision_tail_cut: true,
    };
    let item = RelevanceItem {
        title: Some(title.to_string()),
        ..Default::default()
    };
    filter_relevant_raw(&[item], &query, &settings).kept == vec![0]
}

#[test]
fn artigo_omitido_pelo_release_ainda_e_a_obra() {
    let rejuvenator = ["The Rejuvenator", "Rejuvenatrix"];
    assert!(keeps(&rejuvenator, "1988", "Rejuvenator 1988 DVDRip"));
    assert!(keeps(&["The Matrix"], "1999", "Matrix 1999 1080p BluRay"));
    assert!(keeps(&["The Matrix"], "1999", "Matrix (1999) 720p"));
    assert!(keeps(
        &["The Terminator"],
        "1984",
        "Terminator 1984 1080p BluRay"
    ));
}

#[test]
fn artigo_omitido_nao_abre_outra_obra() {
    // Outra obra com o mesmo primeiro nome e o mesmo ano: a cauda condena.
    assert!(!keeps(
        &["The Invite"],
        "2026",
        "Invite the Devil to Come and Play (2026) 1080p WEBRip"
    ));
    assert!(!keeps(
        &["The Batman"],
        "2022",
        "Batman e Superman Batalha dos Super Filhos (2022) 1080p"
    ));
    // "die" não é artigo omitível: Die Hard não vira "Hard".
    assert!(!keeps(&["Die Hard"], "1988", "Hard 1988 1080p BluRay"));
    // Ano errado continua errado.
    assert!(!keeps(&["The Matrix"], "1999", "Matrix 2021 1080p"));
}

#[test]
fn aka_antes_do_ano_e_titulo_alternativo_da_mesma_obra() {
    let rejuvenator = ["The Rejuvenator", "Rejuvenatrix"];
    assert!(keeps(
        &rejuvenator,
        "1988",
        "Rejuvenatrix (aka The Rejuvenator) VHSRip 1988"
    ));
    assert!(keeps(
        &rejuvenator,
        "1988",
        "Rejuvenatrix(aka The Rejuvenator) VHSRip 1988 [GS]"
    ));
    assert!(keeps(
        &["The Terminator"],
        "1984",
        "The Terminator (aka Terminator) 1984 DVDRip"
    ));
    // Sem "aka", palavra estranha entre o nome curto e o ano segue condenando.
    assert!(!keeps(&["Monster"], "2021", "Monster Hunter 2021 1080p"));
    assert!(!keeps(
        &rejuvenator,
        "1988",
        "Rejuvenatrix Strikes Back 1988 DVDRip"
    ));
}
