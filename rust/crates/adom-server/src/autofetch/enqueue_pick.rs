//! Seleção dos candidatos do lote para a cascata — a parte PURA do laço de
//! `enqueue.rs` (pool, `adrm`, cache, morto e o teto imediato).
//!
//! Separado pela catraca de linhas e por clareza: aqui não há escrita, só as
//! perguntas ao KV (`is_dead`) e a decisão de quem entra.
//!
//! # Pool `seeds` (P4)
//!
//! O lote nativo chega com TODOS os streams elegíveis da obra
//! (`orchestrator/effects.rs` não pré-seleciona pool nenhum), então a escolha do
//! pool `seeds` é feita aqui — é o `pickSeedsPool` do build
//! (`src/providers/autofetch-seeds-pool.ts:58-143`) rodando no escritor único.
//! As réguas (`minSeeders`, `ptFirst`, `rareThreshold`, `rareMaxSeeders`) vêm
//! de [`LivePolicy`] e a decisão é delegada a
//! [`super::super::enqueue_items::seeds_pick`] → `adom_core::pick_seeds_pool`: a
//! MESMA função que o bridge do Node usa, nunca um filtro paralelo.

use std::collections::{HashMap, HashSet};

use adom_autofetch::v3::{CascadeCandidate, CascadePool, QueueV3};
use adom_core::{seeds_intrinsic_rejection, SeedsCandidate, SeedsPolicyConfig};
use adom_index::NativeKv;

use super::super::enqueue_items::{seeds_pick, Item, SeedsKnobs};
use super::gates::LivePolicy;

/// Contexto do lote já medido pelo chamador (holds, cache, `adrm`).
pub(super) struct PickInput<'a> {
    pub(super) kv: &'a NativeKv,
    pub(super) adapter: &'a str,
    pub(super) account: &'a str,
    pub(super) live: &'a LivePolicy,
    pub(super) items: &'a [Item],
    /// Hashes com hold desta chamada (só eles contam como fatos).
    pub(super) eligible: &'a HashSet<&'a str>,
    pub(super) cached: &'a HashSet<String>,
    /// Hashes marcados pelo anti-reenchimento (`adrm:v1`).
    pub(super) adrm: &'a HashSet<String>,
    pub(super) season: Option<i64>,
    pub(super) now_ms: i64,
}

/// Saída da seleção: o mapa hash→item das tentativas, os candidatos na ordem do
/// lote e o teto imediato (`facts.slots`).
pub(super) struct Picked<'a> {
    pub(super) by_hash: HashMap<&'a str, &'a Item>,
    pub(super) candidates: Vec<CascadeCandidate>,
    pub(super) slots: i64,
}

pub(super) fn pick(input: PickInput<'_>) -> Result<Picked<'_>, String> {
    let live = input.live;
    let queue = QueueV3::new(input.kv);
    // Universo do pick de seeds: só o que esta chamada pode tentar (hold vivo,
    // pool `seeds`, sem `adrm`, pool ligado). Fora isso a régua mediria
    // concorrentes que não são candidatos e o regime raro viraria "comum".
    let seeds_items: Vec<&Item> = input
        .items
        .iter()
        .filter(|item| input.eligible.contains(item.info_hash.as_str()))
        .filter(|item| item.pool() == CascadePool::Seeds)
        .filter(|item| !input.adrm.contains(&item.info_hash))
        .filter(|_| live.top_seeds_enabled)
        .filter(|item| seeds_fits(item, live))
        .collect();
    let seeds = seeds_pick(
        &seeds_items,
        &SeedsKnobs {
            min_seeders: live.min_seeders,
            top_seeds_max: live.top_seeds_max,
            pt_first: live.seeds_pt_first,
            rare_max: live.rare_max,
            rare_threshold: live.rare_threshold,
            rare_max_seeders: live.rare_max_seeders,
            queue_depth: live.queue_depth as i64,
        },
        input.season,
    );
    // Uma linha por lote com pool `seeds`: sem ela não dá para saber por que o
    // Chupim parou em N (teto, raro, piso) olhando só a conta do serviço.
    if !seeds_items.is_empty() {
        let fotos = seeds_items.iter().filter(|item| item.from_snapshot).count();
        eprintln!(
            "[autofetch] seeds: {} no pool ({fotos} do índice), {} escolhido(s), teto {}{}",
            seeds_items.len(),
            seeds.ranked.len(),
            seeds.immediate_limit,
            if seeds.rare { " (raro)" } else { "" }
        );
    }

    let mut by_hash: HashMap<&str, &Item> = HashMap::new();
    let mut candidates = Vec::with_capacity(input.items.len());
    // Posição de cada selecionado no pick — é a ordem de enfileiramento do TS.
    let seeds_rank: HashMap<&str, usize> = seeds
        .ranked
        .iter()
        .enumerate()
        .map(|(rank, hash)| (hash.as_str(), rank))
        .collect();
    for item in input.items {
        if !input.eligible.contains(item.info_hash.as_str()) {
            continue;
        }
        // `adrm` vence o filtro de pool: o hash sai ROTULADO, não sumido.
        let adrm = input.adrm.contains(&item.info_hash);
        let seeds_pool = item.pool() == CascadePool::Seeds;
        if !adrm
            && ((item.pool() == CascadePool::Any && !live.any_dubbed_enabled)
                || (seeds_pool && !live.top_seeds_enabled))
        {
            continue;
        }
        // Fora do `pickSeedsPool` não é candidato (o TS nem o devolve).
        if !adrm && seeds_pool && !seeds_rank.contains_key(item.info_hash.as_str()) {
            continue;
        }
        by_hash.insert(item.info_hash.as_str(), item);
        candidates.push(CascadeCandidate {
            info_hash: item.info_hash.clone(),
            pool: item.pool(),
            quality: item.quality.clone(),
            // `slotLimit`/`rare` do lote vêm decididos (`_rare`/`_slotLimit`);
            // ausentes, o regime RARO é o que o pick mediu agora
            // (`autofetch-seeds-pool.ts:101-121`).
            slot_limit: item.slot_limit,
            rare: if item.rare_decided {
                item.rare
            } else {
                seeds_pool && seeds.rare
            },
            cached: input.cached.contains(&item.info_hash),
            dead: queue.is_dead(input.adapter, input.account, &item.info_hash, input.now_ms)?,
            adrm,
            // O piso de seeders já foi aplicado no pick; o waiver é a exceção.
            viable: !item.waived,
        });
    }

    // Ordem do pick: o TS enfileira o melhor swarm/PT primeiro
    // (`pickSeedsPool` devolve os candidatos já ordenados). Sem isso o item
    // fraco que veio antes no lote ocuparia a vaga imediata. `sort_by_key` é
    // estável: os demais pools mantêm a ordem de entrada (o `cascade` os
    // agrupa por prioridade de qualquer forma).
    candidates.sort_by_key(|candidate| {
        seeds_rank
            .get(candidate.info_hash.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });

    // Teto imediato do pool seeds: raro (poucos viáveis E fracos) sobe de
    // `topSeedsMax` para `rareMax` — o mesmo número que o pick devolve
    // (`immediateLimit`, `autofetch-seeds-pool.ts:101-121`).
    let slots = if candidates
        .iter()
        .filter(|candidate| !candidate.dead && candidate.viable && !candidate.adrm)
        .all(|candidate| candidate.pool == CascadePool::Seeds)
    {
        seeds.immediate_limit
    } else {
        live.max_attempts
    };
    Ok(Picked {
        by_hash,
        candidates,
        slots,
    })
}

/// Limites intrínsecos do último nível (qualidade e tamanho): a régua do
/// núcleo que o oráculo já aplicava e o worker nativo não — um 1080p de 20 GB
/// do índice entrava no TorBox (Night Life 1989, 2026-11-16).
fn seeds_fits(item: &Item, live: &LivePolicy) -> bool {
    let cfg = SeedsPolicyConfig {
        dubbed_only: false,
        cached_only: false,
        max_size_gb: 0.0,
        seeds_max_gb: live.seeds_max_gb,
        seeds_max_quality: live.seeds_max_quality.to_string(),
    };
    let candidate = SeedsCandidate {
        size_field: item.size.map(|bytes| bytes as f64),
        pack_bytes: item.pack_bytes.map(|bytes| bytes as f64),
        quality_underscore: item.quality.clone(),
        quality_plain: None,
        size_queue: None,
        title: item.title.clone(),
        name: item.name.clone(),
    };
    seeds_intrinsic_rejection(&candidate, &cfg).is_none()
}

#[cfg(test)]
#[path = "enqueue_pick_tests.rs"]
mod tests;
