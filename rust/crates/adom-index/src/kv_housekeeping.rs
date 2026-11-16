//! Faxina do boot do `NativeKv`: versão velha, prefixo aposentado e vencido
//! sem dono. Extraída de `kv.rs` pela catraca de 400 linhas (2026-11-15);
//! mesma régua, sem mudança de comportamento.

use rusqlite::params;

use super::NativeKv;

/// Sem escritor nos dois lados, mas ainda na tabela do núcleo: `LEGACY_PREFIXES`
/// é conferido por golden, e estes só precisam sair do disco.
const RETIRED_PREFIXES: &[&str] = &["rdt:", "dinv:"];

/// Namespaces cujo vencido ninguém mais poda. A lista é fechada de propósito:
/// um namespace novo com dono nunca entra aqui por omissão. Os donos do segundo
/// bloco só apagam vencido na chave que releem, e `adrm`/`adsub` raramente são
/// relidos; para todos os leitores deles, vencido e ausente já são a mesma coisa.
const UNOWNED_EXPIRY_PREFIXES: &[&str] = &[
    "adrm:",
    "adsub:",
    "adhold:",
    "autofetch:",
    "autofetch-worker:",
    "harvest:",
    "tmdb:",
    "tmdbc:",
    "tmdbh:",
    "meta:",
    "vres:",
    "cfg:",
    "seed:",
    "notify:",
    "indexer-status:",
];

/// Linhas removidas pela faxina do boot, por motivo.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BootHousekeeping {
    pub stale: usize,
    pub legacy: usize,
    pub expired: usize,
}

impl NativeKv {
    /// Faxina do boot do gateway, herdada da poda que o Node fazia no mesmo
    /// arquivo: versão velha, prefixo aposentado e vencido dos namespaces sem
    /// dono com ciclo próprio. Vencido de namespace com dono fica com o dono.
    ///
    /// Só apaga versão MENOR que a da tabela do núcleo, e `idx` fica de fora:
    /// o `IdxStore` cuida da própria migração.
    pub fn boot_housekeeping(&self, now_ms: i64) -> Result<BootHousekeeping, String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|err| format!("início da faxina: {err}"))?;
        let mut done = BootHousekeeping::default();
        for (namespace, version) in adom_core::NAMESPACE_VERSIONS {
            let current = version
                .strip_prefix('v')
                .and_then(|v| v.parse::<i64>().ok());
            let Some(current) = current.filter(|_| *namespace != "idx") else {
                continue;
            };
            done.stale += tx
                .execute(
                    "DELETE FROM cache WHERE key LIKE ?1 AND CAST(substr(key, ?2) AS INTEGER) < ?3",
                    params![format!("{namespace}:v%"), namespace.len() + 3, current],
                )
                .map_err(|err| format!("faxina de versão: {err}"))?;
        }
        for prefix in adom_core::LEGACY_PREFIXES.iter().chain(RETIRED_PREFIXES) {
            done.legacy += tx
                .execute(
                    "DELETE FROM cache WHERE substr(key, 1, length(?1)) = ?1",
                    params![prefix],
                )
                .map_err(|err| format!("faxina de legado: {err}"))?;
        }
        for prefix in UNOWNED_EXPIRY_PREFIXES {
            done.expired += tx
                .execute(
                    "DELETE FROM cache
                     WHERE expires_at > 0 AND expires_at <= ?1
                       AND substr(key, 1, length(?2)) = ?2",
                    params![now_ms, prefix],
                )
                .map_err(|err| format!("faxina de vencido: {err}"))?;
        }
        tx.commit().map_err(|err| format!("fim da faxina: {err}"))?;
        Ok(done)
    }
}
