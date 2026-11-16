//! Controle do operador sobre o raspador (pausa, liga/desliga, ritmo e teto).
//!
//! Antes os botões do painel só mudavam o estado do Node: o sidecar nunca os
//! recebia e o `set_paused` do motor não tinha chamador (medido em
//! 2026-11-15: o painel da VPS mostrava "desligado, só Vaca" enquanto o
//! sidecar raspava os nove sites). Agora o sidecar é o dono desse estado:
//! o Node encaminha as ações por `POST /crawl/control` e o painel o lê por
//! `GET /crawl/control`.
//!
//! O estado sobrevive ao restart (`crawl_state`, site [`CONTROL_SITE`]); sem
//! nada gravado vale o padrão de sempre — tudo ligado, ritmo e teto do `.env`.
//!
//! Quem obedece: o tick dos sites HTML (pausa/desligado global, site fora do
//! lote, teto por hora do site), a lane do Mico, a descoberta e o executor de
//! portões do BLUDV.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Site reservado do `crawl_state` onde o estado é gravado.
pub const CONTROL_SITE: &str = "__control__";
/// Chave do estado no `crawl_state`.
pub const CONTROL_KEY: &str = "control:v1";
const HOUR_MS: i64 = 3_600_000;
/// Limites do que o painel pode pedir (o mesmo intervalo do schema do Node).
pub const MAX_DELAY_MS: u64 = 600_000;
pub const MAX_PER_HOUR: u32 = 100_000;

/// Controle de UM site. `None` = segue o global/padrão.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteControl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_hour: Option<u32>,
}

/// Estado completo (o que é gravado e o que o `GET` devolve).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    #[serde(default = "ligado")]
    pub enabled: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_hour: Option<u32>,
    #[serde(default)]
    pub sites: BTreeMap<String, SiteControl>,
}

fn ligado() -> bool {
    true
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            enabled: true,
            paused: false,
            delay_ms: None,
            max_per_hour: None,
            sites: BTreeMap::new(),
        }
    }
}

/// Pedido do painel. `site` ausente = global. Campo ausente não muda nada;
/// `null` em `enabled`/`delayMs`/`maxPerHour` apaga o override; `reset` volta
/// o alvo ao padrão (sem `site`: tudo).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlPatch {
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub paused: Option<bool>,
    #[serde(default, deserialize_with = "nullable")]
    pub enabled: Option<Option<bool>>,
    #[serde(default, deserialize_with = "nullable")]
    pub delay_ms: Option<Option<u64>>,
    #[serde(default, deserialize_with = "nullable")]
    pub max_per_hour: Option<Option<u32>>,
    #[serde(default)]
    pub reset: bool,
}

/// Distingue campo ausente (`None`) de `null` (`Some(None)`).
fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Estado vivo + contagem por hora de cada site (memória: zera no restart).
#[derive(Debug, Default)]
pub struct CrawlControl {
    state: Mutex<ControlState>,
    hours: Mutex<HashMap<String, (i64, u32)>>,
}

impl CrawlControl {
    pub fn new(state: ControlState) -> Self {
        Self {
            state: Mutex::new(state),
            hours: Mutex::default(),
        }
    }

    pub fn snapshot(&self) -> ControlState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn replace(&self, state: ControlState) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }

    /// Aplica o pedido e devolve o estado novo. Erro = pedido inválido (400).
    pub fn apply(&self, patch: &ControlPatch) -> Result<ControlState, String> {
        validate(patch)?;
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = guard.clone();
        match patch.site.as_deref().map(str::trim) {
            Some(site) if !site.is_empty() => {
                if patch.reset {
                    next.sites.remove(site);
                } else {
                    let entry = next.sites.entry(site.to_string()).or_default();
                    if let Some(paused) = patch.paused {
                        entry.paused = paused;
                    }
                    if let Some(enabled) = patch.enabled {
                        entry.enabled = enabled;
                    }
                    if let Some(delay) = patch.delay_ms {
                        entry.delay_ms = delay;
                    }
                    if let Some(cap) = patch.max_per_hour {
                        entry.max_per_hour = cap;
                    }
                    if *entry == SiteControl::default() {
                        next.sites.remove(site);
                    }
                }
            }
            Some(_) => return Err("site vazio".into()),
            None if patch.reset => next = ControlState::default(),
            None => {
                if let Some(paused) = patch.paused {
                    next.paused = paused;
                }
                if let Some(enabled) = patch.enabled {
                    next.enabled = enabled.unwrap_or(true);
                }
                if let Some(delay) = patch.delay_ms {
                    next.delay_ms = delay;
                }
                if let Some(cap) = patch.max_per_hour {
                    next.max_per_hour = cap;
                }
            }
        }
        *guard = next.clone();
        Ok(next)
    }

    /// O raspador inteiro está parado (desligado ou pausado)?
    pub fn stopped(&self) -> Option<&'static str> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.enabled {
            Some("desligado")
        } else if state.paused {
            Some("pausado")
        } else {
            None
        }
    }

    /// O site não pode trabalhar agora (parado, desligado, pausado ou no teto)?
    pub fn site_blocked(&self, site: &str, now_ms: i64) -> bool {
        if self.stopped().is_some() || self.site_off(site) {
            return true;
        }
        let Some(cap) = self.site_cap(site) else {
            return false;
        };
        self.pages_this_hour(site, now_ms) >= cap
    }

    /// Sites com pausa/desligado próprio ou no teto: ficam fora do lote.
    pub fn blocked_sites(&self, now_ms: i64) -> Vec<String> {
        let ids: Vec<String> = self.snapshot().sites.into_keys().collect();
        ids.into_iter()
            .filter(|site| self.site_blocked(site, now_ms))
            .collect()
    }

    /// Pausa ou desligado do PRÓPRIO site (sem olhar teto nem global).
    pub fn site_off(&self, site: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .sites
            .get(site)
            .is_some_and(|s| s.paused || s.enabled == Some(false))
    }

    /// Teto global do processo (override do painel ou o do `.env`).
    pub fn global_cap(&self, default_cap: u32) -> u32 {
        self.snapshot().max_per_hour.unwrap_or(default_cap)
    }

    /// Ritmo do site: override do site → override global → o do `.env`.
    pub fn delay_for(&self, site: &str, default_ms: u64) -> u64 {
        let state = self.snapshot();
        state
            .sites
            .get(site)
            .and_then(|s| s.delay_ms)
            .or(state.delay_ms)
            .unwrap_or(default_ms)
    }

    fn site_cap(&self, site: &str) -> Option<u32> {
        self.snapshot().sites.get(site).and_then(|s| s.max_per_hour)
    }

    /// Conta UMA página do site na janela de 1 h dele.
    pub fn note_page(&self, site: &str, now_ms: i64) {
        let mut hours = self.hours.lock().unwrap_or_else(|e| e.into_inner());
        let slot = hours.entry(site.to_string()).or_insert((now_ms, 0));
        if now_ms - slot.0 >= HOUR_MS {
            *slot = (now_ms, 0);
        }
        slot.1 = slot.1.saturating_add(1);
    }

    pub fn pages_this_hour(&self, site: &str, now_ms: i64) -> u32 {
        let hours = self.hours.lock().unwrap_or_else(|e| e.into_inner());
        match hours.get(site) {
            Some((start, count)) if now_ms - start < HOUR_MS => *count,
            _ => 0,
        }
    }
}

fn validate(patch: &ControlPatch) -> Result<(), String> {
    if let Some(Some(delay)) = patch.delay_ms {
        if delay > MAX_DELAY_MS {
            return Err(format!("delayMs fora de 0..{MAX_DELAY_MS}"));
        }
    }
    if let Some(Some(cap)) = patch.max_per_hour {
        if cap == 0 || cap > MAX_PER_HOUR {
            return Err(format!("maxPerHour fora de 1..{MAX_PER_HOUR}"));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
