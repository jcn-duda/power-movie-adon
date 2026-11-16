use super::super::link_memo::{run_bounded, wanted_text, BoxedTask};
use super::super::types::JackettRawItem;
use super::JackettClient;
use crate::http::{now_ms, remaining};

/// Abaixo disso o salto não cabe: nem começa.
const MIN_RESOLVE_BUDGET_MS: u64 = 400;

impl JackettClient {
    /// `Link` → magnet dos itens sem hash/magnet, em paralelo
    /// (`JACKETT_RESOLVE_CONCURRENCY`) e com memo entre buscas. O teto
    /// `JACKETT_MAX_DOWNLOAD_RESOLVES` conta só ida à rede; memo não gasta.
    /// A ordem de saída é a de entrada.
    pub(super) async fn resolve_links(
        &self,
        indexer: &str,
        query: &str,
        items: Vec<JackettRawItem>,
        deadline_at: i64,
    ) -> Vec<JackettRawItem> {
        let now = now_ms();
        let wanted = wanted_text(query);
        let mut slots: Vec<Option<JackettRawItem>> = Vec::with_capacity(items.len());
        let mut pending: Vec<(usize, String, bool)> = Vec::new();
        for item in items {
            let has_hash = item.info_hash.as_deref().is_some_and(|h| !h.is_empty());
            let has_magnet = item
                .magnet
                .as_deref()
                .is_some_and(|m| m.to_ascii_lowercase().starts_with("magnet:?"));
            if !(has_hash || has_magnet) {
                if let Some(link) = item.download_url.clone() {
                    // O que nomeia a obra pede o salto antes: o teto não pode
                    // ser gasto com post só parecido.
                    let named = !wanted.is_empty()
                        && adom_core::normalize_title(&item.title).contains(&wanted);
                    pending.push((slots.len(), link, named));
                }
            }
            slots.push(Some(item));
        }
        if pending.is_empty() {
            return slots.into_iter().flatten().collect();
        }
        pending.sort_by_key(|(_, _, named)| !*named);

        let mut network: Vec<(usize, String)> = Vec::new();
        let (mut from_memo, mut memo_misses, mut over_cap) = (0usize, 0usize, 0usize);
        for (index, link, _) in pending {
            match self.link_memo.get(&link, now) {
                Some(Some(magnet)) => {
                    if let Some(item) = slots[index].as_mut() {
                        item.magnet = Some(magnet);
                    }
                    from_memo += 1;
                }
                Some(None) => {
                    slots[index] = None;
                    memo_misses += 1;
                }
                None if network.len() < self.config.max_download_resolves => {
                    network.push((index, link));
                }
                None => {
                    slots[index] = None;
                    over_cap += 1;
                }
            }
        }

        let full_budget = self.config.download_timeout_ms;
        let tasks: Vec<BoxedTask<'_, (Option<String>, bool)>> = network
            .iter()
            .map(|(_, link)| {
                let link = link.as_str();
                Box::pin(async move {
                    let left = remaining(deadline_at)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if left < MIN_RESOLVE_BUDGET_MS {
                        return (None, false);
                    }
                    let magnet = self.resolve_download_magnet(link, left).await;
                    // Negativo só com o prazo cheio: corte pelo deadline da
                    // busca não prova que o Link é ruim.
                    (magnet, left >= full_budget)
                }) as BoxedTask<'_, (Option<String>, bool)>
            })
            .collect();
        let outcomes = run_bounded(tasks, self.config.resolve_concurrency).await;

        let (mut resolved, mut failed) = (0usize, 0usize);
        let stamp = now_ms();
        for ((index, link), (magnet, full)) in network.iter().zip(outcomes) {
            match magnet {
                Some(magnet) => {
                    self.link_memo.put(link, Some(magnet.clone()), stamp);
                    if let Some(item) = slots[*index].as_mut() {
                        item.magnet = Some(magnet);
                    }
                    resolved += 1;
                }
                None => {
                    if full {
                        self.link_memo.put(link, None, stamp);
                    }
                    slots[*index] = None;
                    failed += 1;
                }
            }
        }
        if failed + memo_misses + over_cap > 0 || resolved + from_memo > 0 {
            eprintln!(
                "[jackett] {indexer}: Link→magnet {resolved} na rede, {from_memo} do memo; \
                 sem magnet útil {failed} (+{memo_misses} falha recente, +{over_cap} além do teto)"
            );
        }
        slots.into_iter().flatten().collect()
    }
}
