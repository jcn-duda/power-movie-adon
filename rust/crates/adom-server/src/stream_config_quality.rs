//! Filtro e tetos de qualidade do operador (`QUALITY_FILTER`,
//! `QUALITY_RELAX_BELOW`, `MAX_STREAMS_*`), aplicados sobre o default do build.

use adom_search::BuildOptions;

use super::env_f64;

pub(super) fn apply_quality_env(b: &mut BuildOptions) {
    // Ausente → default HD; `QUALITY_FILTER=` vazio de propósito aceita todas.
    if let Ok(raw) = std::env::var("QUALITY_FILTER") {
        b.qualities = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    // `QUALITY_RELAX_BELOW=0` volta à régua do TS (SD só quando nada passa).
    if let Some(n) = env_f64("QUALITY_RELAX_BELOW") {
        b.quality_relax_below = n.max(0.0);
    }
    let mut limits = b.quality_limits.clone();
    // Chave do balde sem resolução = `UNKNOWN_QUALITY` do núcleo ("sem resolução").
    for (env_key, quality) in [
        ("MAX_STREAMS_2160P", "2160p"),
        ("MAX_STREAMS_1080P", "1080p"),
        ("MAX_STREAMS_720P", "720p"),
        ("MAX_STREAMS_480P", "480p"),
        ("MAX_STREAMS_SD", "SD"),
        ("MAX_STREAMS_UNKNOWN", "sem resolução"),
    ] {
        if let Some(n) = env_f64(env_key) {
            limits.insert(quality.to_string(), n);
        }
    }
    b.quality_limits = limits;
}
