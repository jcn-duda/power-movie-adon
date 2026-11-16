//! Regras vivas do laço em RAM: pausa/intervalo, contadores do painel e o
//! `Notify` que acorda a thread.
//!
//! Separado de `serve.rs` por causa da catraca de linhas (teto 400) e porque
//! este é o lado que o PAINEL lê: `POST /autofetch/config` persiste no KV e
//! espelha aqui, então o laço nunca toca SQLite para saber se está pausado.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::Notify;

/// Contadores VIVOS medidos no fim de uma passagem (fila e lotes no KV). O
/// `status()` do painel lê daqui — nunca SQLite.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LiveCounts {
    pub(crate) queue_depth: usize,
    pub(crate) lots: usize,
}

/// Retrato dos contadores para o painel (`GET /autofetch`).
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    pub(crate) inflight: bool,
    pub(crate) processed: u64,
    pub(crate) skips: BTreeMap<String, usize>,
    pub(crate) last_error: Option<String>,
    /// Contadores vivos da última passagem; `None` = sem laço (prepared/stub),
    /// e o status cai na foto do boot (`Hydrated`).
    pub(crate) live_counts: Option<LiveCounts>,
}

/// Regras vivas em RAM: pausa/intervalo que o laço lê a cada volta, mais os
/// contadores.
pub(crate) struct Control {
    paused: AtomicBool,
    interval_ms: AtomicU64,
    inflight: AtomicUsize,
    processed: AtomicU64,
    last_error: Mutex<Option<String>>,
    skips: Mutex<BTreeMap<String, usize>>,
    live_counts: Mutex<Option<LiveCounts>>,
    notify: Notify,
}

impl Control {
    pub(crate) fn new(paused: bool, interval_ms: u64) -> Self {
        Self {
            paused: AtomicBool::new(paused),
            interval_ms: AtomicU64::new(interval_ms.max(1)),
            inflight: AtomicUsize::new(0),
            processed: AtomicU64::new(0),
            last_error: Mutex::new(None),
            skips: Mutex::new(BTreeMap::new()),
            live_counts: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    pub(crate) fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub(crate) fn interval_ms(&self) -> u64 {
        self.interval_ms.load(Ordering::Relaxed).max(1)
    }

    pub(crate) fn set_interval_ms(&self, interval_ms: u64) {
        self.interval_ms
            .store(interval_ms.max(1), Ordering::Relaxed);
    }

    /// Acorda o laço (chegou trabalho ou o painel mexeu na config).
    pub(crate) fn wake(&self) {
        self.notify.notify_one();
    }

    pub(crate) async fn wait(&self) {
        self.notify.notified().await;
    }

    pub(crate) fn enter(&self) {
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn leave(&self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub(crate) fn inflight(&self) -> bool {
        self.inflight.load(Ordering::Relaxed) > 0
    }

    /// Contadores da passagem. `errors` é o do painel: a mensagem nasce de
    /// store/rede e o cofre não entra nela (nenhum adaptador devolve chave).
    pub(crate) fn record(&self, jobs: usize, skips: &BTreeMap<String, usize>, errors: &[String]) {
        self.processed.fetch_add(jobs as u64, Ordering::Relaxed);
        let mut current = self.skips.lock().unwrap_or_else(PoisonError::into_inner);
        for (label, count) in skips {
            *current.entry(label.clone()).or_default() += count;
        }
        if let Some(first) = errors.first() {
            *self
                .last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(first.clone());
        }
    }

    /// Contadores vivos do painel: o laço publica a fila e os lotes medidos no
    /// fim de CADA passagem — a foto do boot não é o estado atual.
    pub(crate) fn publish_counts(&self, queue_depth: usize, lots: usize) {
        *self
            .live_counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(LiveCounts { queue_depth, lots });
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            inflight: self.inflight(),
            processed: self.processed.load(Ordering::Relaxed),
            skips: self
                .skips
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            live_counts: *self
                .live_counts
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        }
    }
}
