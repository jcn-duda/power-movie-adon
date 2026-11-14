//! Porte de `isHomeSource`, `preHomeReleaseContradicts` e `isNonFeature`
//! (`src/utils/non-feature-release.ts`).
//!
//! O corte antes do lançamento doméstico não lê config: o chamador passa
//! `cut_enabled` e `margin_ms`. No TS isso é `config.search.preHomeReleaseCut`
//! (default true) e `preHomeReleaseMarginMs` (default 48h).

use std::sync::OnceLock;

use regex::Regex;

use crate::br_origin::js_canon;

// `/i` sem `u`: o texto sobe por `js_canon` e o `\b` é ASCII (`(?-u:\b)`).
// Um acento não é palavra no JS, então "éweb" tem fronteira antes do W.
// SCR/SCREENER e "HdTScr" também são cinema ("Godzilla Minus Zero (2026)
// HdTScr Lat"); HQ é o prefixo de "HQCAM".
const CINEMA: &str = r"(?-u:\b)(?:(?:HD|HQ)[-. ]?)?(?:CAM(?:[-. ]?RIP)?|TS(?:[-. ]?S?CR)?|TC|TELESYNC|TELECINE|PRE[-. ]?DVD|(?:DVD[-. ]?)?SCR(?:EENER)?)(?-u:\b)";
const HOME: &str = r"(?-u:\b)(?:WEB(?:[-. ]?(?:DL|RIP))?|BLU[-. ]?RAY|BD(?:RIP|REMUX)?|BRRIP|REMUX|HDTV|DVD(?:RIP|R|5|9)?)(?-u:\b)";
// Featurette fica de fora de propósito. O release "BluRay + Featurettes"
// traz o filme junto do extra; marcar featurette condenaria o filme. O
// regex do TS é só trailer/teaser/trilha (`trailers?`, `teasers?`,
// `soundtrack`, `OST`).
const NON_FEATURE: &str = r"(?-u:\b)(?:TRAILERS?|TEASERS?|SOUNDTRACK|OST)(?-u:\b)";

fn compiled(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex de non-feature compila"))
}

fn cinema_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    compiled(&RE, CINEMA)
}

fn home_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    compiled(&RE, HOME)
}

fn non_feature_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    compiled(&RE, NON_FEATURE)
}

/// `_` vira espaço, como o `clean` do TS, e a caixa segue o `/i` sem `u`.
fn cleaned(text: &str) -> String {
    js_canon(&text.replace('_', " "))
}

/// O nome declara fonte doméstica (WEB/BluRay/HDTV/DVD) e não é gravação de
/// cinema. "TS-V2-WEB.DL" casa os dois: o TS é palavra, o WEB.DL também. O
/// cinema anula a fonte — o marcador de sala vence, e a release passa.
pub fn is_home_source(text: &str) -> bool {
    let t = cleaned(text);
    !cinema_re().is_match(&t) && home_re().is_match(&t)
}

/// Trailer, teaser ou trilha que o nome da obra não explica. Se o nome já
/// contém a palavra ("The Trailer Park"), não é extra: devolve false.
pub fn is_non_feature(texts: &[&str], names: &[&str]) -> bool {
    if names.iter().any(|n| non_feature_re().is_match(&cleaned(n))) {
        return false;
    }
    texts
        .iter()
        .any(|t| !t.is_empty() && non_feature_re().is_match(&cleaned(t)))
}

/// Fonte doméstica antes do 1º lançamento doméstico, com margem. `None` é
/// null/undefined; `0` é falsy no JS (`!homeReleaseAt`). NaN e infinito não
/// cabem em `i64` — o TS também recusa com `Number.isFinite`.
pub fn pre_home_release_contradicts(
    texts: &[&str],
    home_release_at: Option<i64>,
    now: i64,
    cut_enabled: bool,
    margin_ms: i64,
) -> bool {
    if !cut_enabled {
        return false;
    }
    if !in_pre_home_window(home_release_at, now, margin_ms) {
        return false;
    }
    texts.iter().any(|t| !t.is_empty() && is_home_source(t))
}

/// `None` é null/undefined e `0` é falsy no JS (`!homeReleaseAt`).
/// `homeReleaseAt - now <= margin` em i128: não estoura i64 e, nestes
/// milissegundos, coincide com o número do JS.
fn in_pre_home_window(home_release_at: Option<i64>, now: i64, margin_ms: i64) -> bool {
    let Some(at) = home_release_at.filter(|n| *n != 0) else {
        return false;
    };
    (at as i128) - (now as i128) > margin_ms as i128
}

/// Na mesma janela, release GLOBAL que não declara gravação de cinema não é o
/// filme. Antes do doméstico só existe cópia de sala; o que se diz "1080p HD
/// h264" sem fonte é fake (Godzilla Minus Zero, 2026-11-14: três hashes de
/// "1080p HD h264- ETHEL" com tamanhos diferentes, ⚡ no debrid e nenhum vídeo
/// dentro). Fonte BR fica de fora: o post BR raramente nomeia a fonte e tem o
/// filtro de título estrito.
pub fn pre_home_unsourced_contradicts(
    texts: &[&str],
    is_br: bool,
    home_release_at: Option<i64>,
    now: i64,
    cut_enabled: bool,
    margin_ms: i64,
) -> bool {
    if !cut_enabled || is_br || !in_pre_home_window(home_release_at, now, margin_ms) {
        return false;
    }
    !texts
        .iter()
        .any(|t| !t.is_empty() && cinema_re().is_match(&cleaned(t)))
}
