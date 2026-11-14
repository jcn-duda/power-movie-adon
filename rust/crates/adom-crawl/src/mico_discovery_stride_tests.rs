//! Testes do PASSO DINÂMICO da descoberta do Mico (irmão de
//! `mico_discovery_tests.rs`, catraca de 400 linhas): avanço pelo tamanho
//! real da página, páginas variáveis sem lacuna e dedupe de sobreposição.

use crate::mico::MicoKind;
use crate::mico_net::api::{MicoApi, MicoFuture};
use crate::mico_net::types::{CatalogPage, CatalogType, MicoError};

use super::tests::{cfg, sem_abortar, FakeApi};
use super::{walk_catalog, MicoCrawlConfig};

#[tokio::test]
async fn passo_dinamico_avanca_pelo_tamanho_da_pagina() {
    let api = FakeApi::default().always();
    let cfg = MicoCrawlConfig {
        max_pages: 5,
        empty_retries: 0,
        end_after_empties: 99,
        known_pages_to_stop: 99,
        ..cfg()
    };
    let walk = walk_catalog(&api, &cfg, MicoKind::Movie, None, &sem_abortar()).await;
    assert_eq!(walk.pages, 5);
    assert_eq!(
        api.calls(),
        vec![0, 1, 2, 3, 4],
        "página de 1 meta avança 1, não o passo fixo antigo"
    );
}

/// Páginas de tamanho VARIÁVEL (14–97 metas na API real): o passo acompanha
/// cada página — sem lacuna quando a página é menor que o passo fixo antigo.
#[tokio::test]
async fn paginas_de_tamanho_variavel_nao_criam_lacuna() {
    let api = FakeApi::new(&[
        "tt1000001",
        "tt1000002",
        "tt1000003",
        "tt1000004",
        "tt1000005",
        "tt1000006",
    ]);
    let cfg = MicoCrawlConfig {
        max_pages: 50,
        empty_retries: 0,
        end_after_empties: 1,
        known_pages_to_stop: 99,
        ..cfg()
    };
    let walk = walk_catalog(&api, &cfg, MicoKind::Movie, None, &sem_abortar()).await;
    assert_eq!(api.calls(), vec![0, 6], "fatia de 6 e a sonda de fim");
    assert_eq!(walk.ids.len(), 6, "nenhuma obra ficou para trás");
    assert!(walk.saw_end);
}

/// A API real repete obras entre páginas (catálogo mais-novos-primeiro em
/// churn): o dedupe segura a repetição e o passo avança pelo tamanho BRUTO
/// da página — nunca pelo nº de obras novas (senão orbitaria no mesmo skip).
#[tokio::test]
async fn sobreposicao_entre_paginas_e_deduplicada_sem_orbitar() {
    struct Repetidor;
    impl MicoApi for Repetidor {
        fn catalog_page<'a>(
            &'a self,
            _kind: CatalogType,
            _catalog_id: &'a str,
            skip: usize,
        ) -> MicoFuture<'a, Result<CatalogPage, MicoError>> {
            Box::pin(async move {
                // Duas obras fixas + uma nova por página: sobreposição de 2/3.
                let ids = vec![
                    "tt1000001".to_string(),
                    "tt1000002".to_string(),
                    format!("tt{}", 2_000_000 + skip),
                ];
                Ok(CatalogPage {
                    count: ids.len(),
                    ids,
                })
            })
        }
        fn get_json<'a>(
            &'a self,
            _url: &'a str,
        ) -> MicoFuture<'a, Result<Option<String>, MicoError>> {
            Box::pin(async { unimplemented!() })
        }
        fn series_meta<'a>(
            &'a self,
            _imdb: &'a str,
        ) -> MicoFuture<'a, Result<Option<crate::mico::SeriesMeta>, MicoError>> {
            Box::pin(async { unimplemented!() })
        }
    }
    let cfg = MicoCrawlConfig {
        max_pages: 3,
        empty_retries: 0,
        end_after_empties: 99,
        known_pages_to_stop: 99,
        ..cfg()
    };
    let walk = walk_catalog(&Repetidor, &cfg, MicoKind::Movie, None, &sem_abortar()).await;
    assert_eq!(walk.pages, 3);
    assert_eq!(walk.ids.len(), 5, "2 fixas + 3 novas, sem repetição");
}
