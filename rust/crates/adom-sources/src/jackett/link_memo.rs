//! Memo Link → magnet e execução com teto de concorrência.
//!
//! Indexer que só publica `Link` (MagnetDL, protetores BR) cobra um salto por
//! item (~1–2 s, às vezes 504). Sem memo, cada busca refazia os MESMOS
//! primeiros saltos, o prazo vencia e o resto morria como "Link sem magnet
//! útil" — busca após busca, sempre os mesmos 2 de 10 sobreviviam (Night Life
//! 1989, 2026-11-16). O magnet de um `Link` não muda: o positivo vale um dia.

use std::collections::{HashMap, VecDeque};
use std::future::{poll_fn, Future};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::Poll;

const POSITIVE_TTL_MS: i64 = 24 * 3600 * 1000;
/// Negativo curto: site lento hoje pode responder na próxima busca.
const NEGATIVE_TTL_MS: i64 = 10 * 60 * 1000;
const MAX_ENTRIES: usize = 20_000;

/// Veredito guardado: `Some(magnet)` resolvido, `None` falhou com prazo cheio.
type Entry = (Option<String>, i64);

#[derive(Default)]
pub(crate) struct LinkMemo {
    // Chave é o hash do Link: a URL do `/dl` do Jackett carrega a apikey e
    // não precisa ficar guardada em texto.
    inner: Mutex<HashMap<u64, Entry>>,
}

impl LinkMemo {
    fn key(link: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        link.hash(&mut hasher);
        hasher.finish()
    }

    /// `Some(Some(m))` = magnet memorizado; `Some(None)` = falha recente;
    /// `None` = nunca visto ou vencido.
    pub(crate) fn get(&self, link: &str, now: i64) -> Option<Option<String>> {
        let map = self.inner.lock().ok()?;
        let (value, expires) = map.get(&Self::key(link))?;
        (*expires > now).then(|| value.clone())
    }

    pub(crate) fn put(&self, link: &str, magnet: Option<String>, now: i64) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        if map.len() >= MAX_ENTRIES {
            map.retain(|_, (_, expires)| *expires > now);
            if map.len() >= MAX_ENTRIES {
                map.clear();
            }
        }
        let ttl = if magnet.is_some() {
            POSITIVE_TTL_MS
        } else {
            NEGATIVE_TTL_MS
        };
        map.insert(Self::key(link), (magnet, now + ttl));
    }
}

pub(crate) type BoxedTask<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Roda as tarefas com no máximo `limit` em voo e devolve na ordem de entrada.
pub(crate) async fn run_bounded<T>(tasks: Vec<BoxedTask<'_, T>>, limit: usize) -> Vec<T> {
    let limit = limit.max(1);
    let total = tasks.len();
    let mut waiting: VecDeque<(usize, BoxedTask<'_, T>)> = tasks.into_iter().enumerate().collect();
    let mut running: Vec<(usize, BoxedTask<'_, T>)> = Vec::with_capacity(limit.min(total));
    let mut done: Vec<Option<T>> = (0..total).map(|_| None).collect();
    poll_fn(|cx| loop {
        while running.len() < limit {
            match waiting.pop_front() {
                Some(task) => running.push(task),
                None => break,
            }
        }
        let mut progressed = false;
        let mut i = 0;
        while i < running.len() {
            if let Poll::Ready(value) = running[i].1.as_mut().poll(cx) {
                let (index, _) = running.swap_remove(i);
                done[index] = Some(value);
                progressed = true;
            } else {
                i += 1;
            }
        }
        if running.is_empty() && waiting.is_empty() {
            return Poll::Ready(());
        }
        if !progressed {
            return Poll::Pending;
        }
    })
    .await;
    done.into_iter().flatten().collect()
}

/// Texto da consulta sem ano e sem SxxEyy, normalizado — só para ORDENAR os
/// saltos (o que nomeia a obra vai primeiro); ninguém é descartado aqui.
pub(crate) fn wanted_text(query: &str) -> String {
    let kept: Vec<&str> = query
        .split_whitespace()
        .filter(|token| !is_year(token) && !is_season_token(token))
        .collect();
    adom_core::normalize_title(&kept.join(" "))
}

fn is_year(token: &str) -> bool {
    token.len() == 4
        && (token.starts_with("19") || token.starts_with("20"))
        && token.bytes().all(|b| b.is_ascii_digit())
}

fn is_season_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() < 2 || !(bytes[0] == b'S' || bytes[0] == b's') {
        return false;
    }
    let rest = &token[1..];
    let (season, episode) = match rest.find(['E', 'e']) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
    digits(season) && episode.is_none_or(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_guarda_positivo_e_negativo_com_ttls_distintos() {
        let memo = LinkMemo::default();
        memo.put("a", Some("magnet:?xt=1".into()), 0);
        memo.put("b", None, 0);
        assert_eq!(memo.get("a", 1), Some(Some("magnet:?xt=1".into())));
        assert_eq!(memo.get("b", 1), Some(None));
        assert_eq!(memo.get("b", NEGATIVE_TTL_MS + 1), None);
        assert_eq!(
            memo.get("a", NEGATIVE_TTL_MS + 1),
            Some(Some("magnet:?xt=1".into()))
        );
        assert_eq!(memo.get("c", 1), None);
    }

    #[test]
    fn wanted_tira_ano_e_temporada() {
        assert_eq!(
            wanted_text("Night Life 1989"),
            adom_core::normalize_title("Night Life")
        );
        assert_eq!(
            wanted_text("Dark S01E02"),
            adom_core::normalize_title("Dark")
        );
        assert_eq!(
            wanted_text("Se7en 1995"),
            adom_core::normalize_title("Se7en")
        );
    }

    #[tokio::test]
    async fn run_bounded_preserva_ordem_e_respeita_teto() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<BoxedTask<'_, usize>> = (0..7usize)
            .map(|i| {
                let live = live.clone();
                let peak = peak.clone();
                Box::pin(async move {
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis((7 - i as u64) * 3)).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                    i * 10
                }) as BoxedTask<'_, usize>
            })
            .collect();
        let out = run_bounded(tasks, 3).await;
        assert_eq!(out, vec![0, 10, 20, 30, 40, 50, 60]);
        assert!(peak.load(Ordering::SeqCst) <= 3);
        assert!(peak.load(Ordering::SeqCst) >= 2);
    }
}
