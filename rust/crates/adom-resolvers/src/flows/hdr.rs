//! Busca do HDR Torrents (`resolvers/profiles/hdrtorrents.ts`).
//!
//! Caminho principal: a busca do site (`index.php?busca=`, medida em
//! 2026-11-23), que devolve os mesmos cards `media-card-link` da listagem e
//! cobre o acervo inteiro. Ela é um OR de palavras ordenado por relevância:
//! token extra (SxxEyy, ano, "1ª Temporada") ou artigo ("the boys") empurra a
//! obra para fora da 1ª página. Por isso a query do site vai só com o nome, e
//! uma segunda tentativa sem artigos roda quando a primeira não casa nada.
//! Só a 1ª página: o resto é lixo parecido ("fall*" para "fallout").
//!
//! Fallback: a busca já foi quebrada um dia (o `?s=` é ignorado e devolve os
//! lançamentos), então se ela falhar ou não casar nada o catálogo é raspado
//! pela LISTAGEM (`/`, `/pagina/N/`, ~400 obras recentes) e a query casa
//! localmente. Cache com SWR: vencido, a busca usa o último catálogo bom na
//! hora e o refresh corre em fundo, com prazo longo (ninguém espera); só o
//! cold start paga o prazo curto.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use adom_crawl::net::Allowlist;
use regex::Regex;
use reqwest::header::{ACCEPT, USER_AGENT};
use tokio::sync::Mutex;

use crate::common::{SearchItem, SitePost};
use crate::profile::ResolverProfile;
use crate::search_select;
use crate::sites::hdrtorrents::{parse_listing_html, HdrWork};

/// 20 páginas × 20 cards ≈ 400 obras: o catálogo corrente.
const MAX_LISTING_PAGES: u32 = 20;
/// Prazo da raspagem que uma busca espera (a busca BR tem 20s de teto).
const LISTING_BUDGET: Duration = Duration::from_millis(10_000);
/// Prazo do aquecimento e do refresh de fundo: com 10s o catálogo saía sempre
/// parcial e variava a cada raspagem (medido em produção no TS).
const LISTING_WARM_BUDGET: Duration = Duration::from_millis(120_000);
const LISTING_TTL: Duration = Duration::from_secs(30 * 60);
const LISTING_PARTIAL_TTL: Duration = Duration::from_secs(2 * 60);
/// Página com menos cards que isto (metade do normal) é fim do catálogo.
const END_OF_CATALOG: usize = 10;

#[derive(Default)]
struct Listing {
    items: Vec<HdrWork>,
    partial: bool,
    expires_at: Option<Instant>,
    has_good: bool,
    refreshing: bool,
}

static LISTING: LazyLock<Mutex<Listing>> = LazyLock::new(|| Mutex::new(Listing::default()));

/// O que a raspagem precisa, sem prender o profile (o refresh de fundo é
/// `'static`).
#[derive(Clone)]
struct Scraper {
    base: String,
    allowlist: Allowlist,
    user_agent: String,
    timeout: Duration,
}

impl Scraper {
    fn of(profile: &ResolverProfile) -> Self {
        Self {
            base: profile.selector.url(),
            allowlist: profile.allowlist.clone(),
            user_agent: profile.network.user_agent.clone(),
            timeout: Duration::from_millis(profile.network.timeout_ms),
        }
    }

    async fn page(&self, client: &reqwest::Client, n: u32) -> Result<Vec<HdrWork>, String> {
        let url = if n <= 1 {
            format!("{}/", self.base)
        } else {
            format!("{}/pagina/{n}/", self.base)
        };
        self.allowlist
            .assert_allowed_url(&url)
            .map_err(|e| e.to_string())?;
        let res = client
            .get(&url)
            .header(USER_AGENT, &self.user_agent)
            .header(ACCEPT, "text/html,application/xhtml+xml")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("http_{}", res.status().as_u16()));
        }
        let html = res.text().await.map_err(|e| e.to_string())?;
        parse_listing_html(&html, &self.base)
    }

    /// `scrapeListings`: páginas em ordem até o fim do catálogo, o teto ou o prazo.
    async fn scrape(&self, budget: Duration) -> Result<(Vec<HdrWork>, bool), String> {
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + budget;
        let mut all: Vec<HdrWork> = Vec::new();
        let mut partial = false;
        for n in 1..=MAX_LISTING_PAGES {
            if Instant::now() > deadline {
                partial = true;
                break;
            }
            let items = self.page(&client, n).await?;
            let count = items.len();
            for item in items {
                if !all.iter().any(|seen| seen.url == item.url) {
                    all.push(item);
                }
            }
            if count < END_OF_CATALOG {
                break;
            }
        }
        Ok((all, partial))
    }
}

fn store(listing: &mut Listing, items: Vec<HdrWork>, partial: bool) {
    let ttl = if partial || items.is_empty() {
        LISTING_PARTIAL_TTL
    } else {
        LISTING_TTL
    };
    listing.items = items;
    listing.partial = partial;
    listing.expires_at = Some(Instant::now() + ttl);
    // Só marca "bom" se tem itens — catálogo vazio não é bom.
    listing.has_good = !listing.items.is_empty();
}

/// Refresh em fundo com o prazo longo; erro só vira log.
fn spawn_refresh(scraper: Scraper) {
    tokio::spawn(async move {
        let result = scraper.scrape(LISTING_WARM_BUDGET).await;
        let mut listing = LISTING.lock().await;
        listing.refreshing = false;
        match result {
            Ok((items, partial)) => store(&mut listing, items, partial),
            Err(e) => eprintln!("[resolver] hdrtorrents: refresh em background falhou ({e})"),
        }
    });
}

/// `fetchAllListingsDetailed`: cache → SWR → cold start (lock só para ler/gravar,
/// NUNCA durante o HTTP; buscas concorrentes não bloqueiam a raspagem).
async fn listings(profile: &ResolverProfile) -> Result<(Vec<HdrWork>, bool), String> {
    // 1. Check cache sob lock rápido.
    {
        let listing = LISTING.lock().await;
        if listing.expires_at.is_some_and(|t| t > Instant::now()) {
            return Ok((listing.items.clone(), listing.partial));
        }
        if listing.has_good {
            if !listing.refreshing {
                // SWR: devolve o bom agora, refresca em fundo.
                let items = listing.items.clone();
                let partial = listing.partial;
                drop(listing); // libera lock ANTES do scrape
                spawn_refresh(Scraper::of(profile));
                // Marca refreshing sob lock.
                let mut listing = LISTING.lock().await;
                listing.refreshing = true;
                return Ok((items, partial));
            }
            // Já está refrescando; devolve o que tem.
            return Ok((listing.items.clone(), listing.partial));
        }
    } // lock liberado aqui

    // 2. Cold start: scrape SEM o lock (outras buscas também podem raspar).
    let (items, partial) = Scraper::of(profile).scrape(LISTING_BUDGET).await?;

    // 3. Grava sob lock rápido.
    {
        let mut listing = LISTING.lock().await;
        store(&mut listing, items.clone(), partial);
    }
    Ok((items, partial))
}

/// Aquece o catálogo no boot (`warm()` do TS): a primeira busca já o encontra.
/// Libera o lock ANTES do scrape (pode demorar 120s).
pub fn warm(profile: &ResolverProfile) {
    let scraper = Scraper::of(profile);
    tokio::spawn(async move {
        // Check rápido sob lock.
        {
            let listing = LISTING.lock().await;
            if listing.has_good {
                return;
            }
        } // lock liberado

        // Scrape SEM o lock.
        match scraper.scrape(LISTING_WARM_BUDGET).await {
            Ok((items, partial)) => {
                eprintln!(
                    "[resolver] hdrtorrents: warm → {} item(s){}",
                    items.len(),
                    if partial { " (parcial)" } else { "" }
                );
                // Grava sob lock rápido.
                let mut listing = LISTING.lock().await;
                store(&mut listing, items, partial);
            }
            Err(e) => eprintln!("[resolver] hdrtorrents: warm falhou ({e})"),
        }
    });
}

static SEASON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)\bS\d{1,2}(?:E\d{1,2})?\b").expect("SxxEyy"));
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u)\b(?:19|20)\d{2}\b").expect("ano"));

/// Artigos que, numa query curta, dominam o OR do site ("the boys" → 20 cards
/// com "the" e nenhum The Boys; "boys" → The Boys no topo).
const ARTICLES: &[&str] = &["the", "a", "an", "o", "os", "as", "um", "uma"];

/// Queries que o site recebe, em ordem: só o nome (sem SxxEyy/ano/`:`), e a
/// mesma sem artigos quando sobra algo diferente. Ano só sai se sobrar nome:
/// no filme "1917 2019" o primeiro ano É o título.
pub fn site_queries(query: &str) -> Vec<String> {
    let no_season = SEASON.replace_all(query, " ").replace(':', " ");
    let no_year = YEAR.replace_all(&no_season, " ");
    let bare: Vec<&str> = match no_year.split_whitespace().next() {
        Some(_) => no_year.split_whitespace().collect(),
        None => no_season.split_whitespace().collect(),
    };
    let mut out = Vec::new();
    if bare.is_empty() {
        return out;
    }
    out.push(bare.join(" "));
    let lean: Vec<&str> = bare
        .iter()
        .copied()
        .filter(|t| !ARTICLES.contains(&t.to_lowercase().as_str()))
        .collect();
    if !lean.is_empty() && lean.len() != bare.len() {
        out.push(lean.join(" "));
    }
    out
}

fn to_posts(works: Vec<HdrWork>) -> Vec<SitePost> {
    works
        .into_iter()
        .map(|w| SitePost {
            title: w.title,
            url: w.url,
            poster: w.poster,
            year: w.year,
            kind: w.kind.map(str::to_string),
            ..Default::default()
        })
        .collect()
}

/// Filtro local da query, igual nos dois caminhos (busca e listagem).
struct Selection {
    normalized: String,
    season: Option<u32>,
    year: Option<i32>,
    max_posts: usize,
}

impl Selection {
    fn of(site_name: &str, query: &str) -> Self {
        let rules = search_select::rules_for(site_name);
        Self {
            normalized: search_select::normalize_query(query, rules),
            season: search_select::requested_season(query),
            year: search_select::requested_year(query),
            max_posts: rules.max_posts,
        }
    }

    /// Ano de OUTRA obra sai ANTES do corte, como no `/?s=` dos outros sites:
    /// a busca "coringa" lista "Delírio a Dois (2024)" antes de "Coringa
    /// (2019)", e o corte em 5 tirava o filme pedido da lista.
    fn apply(&self, works: Vec<HdrWork>) -> Vec<SitePost> {
        let posts = search_select::filter_year(to_posts(works), |p| p.title.as_str(), self.year);
        search_select::select_posts(
            posts,
            |p| p.title.as_str(),
            &self.normalized,
            self.season,
            self.max_posts,
        )
    }
}

/// Busca do site: posts já filtrados pela query, ou `None` quando a busca
/// falhou ou não casou nada (aí quem responde é a listagem).
async fn search_site(
    profile: &ResolverProfile,
    query: &str,
    sel: &Selection,
) -> Option<Vec<SitePost>> {
    let base = profile.selector.url();
    for q in site_queries(query) {
        let html = match profile.search_html(&q).await {
            Ok(html) => html,
            Err(e) => {
                eprintln!("[resolver] hdrtorrents: busca do site falhou ({e}); usando a listagem");
                return None;
            }
        };
        let posts = sel.apply(parse_listing_html(&html, &base).unwrap_or_default());
        if !posts.is_empty() {
            profile.selector.note_success();
            return Some(posts);
        }
    }
    None
}

/// `searchPosts` do HDR: busca do site → (fallback) catálogo → match local →
/// conteúdo de cada post.
pub async fn search_items(
    profile: &ResolverProfile,
    query: &str,
) -> Result<Vec<SearchItem>, String> {
    let sel = Selection::of(&profile.site_name, query);
    if let Some(posts) = search_site(profile, query, &sel).await {
        return profile.items_for_posts(posts, sel.season).await;
    }
    let (works, _partial) = match listings(profile).await {
        Ok(found) => {
            profile.selector.note_success();
            found
        }
        Err(e) => {
            profile.account_failure(&e).await;
            return Err(e);
        }
    };
    profile.items_for_posts(sel.apply(works), sel.season).await
}

#[cfg(test)]
#[path = "hdr_tests.rs"]
mod tests;
