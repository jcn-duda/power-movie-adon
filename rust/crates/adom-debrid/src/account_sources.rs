//! Leitores por serviço do inventário da conta (F3).
//!
//! Cada adaptador devolve só o que já está PRONTO na conta, com o mesmo filtro
//! de "pronto" do TS. Nada aqui grava estado: dedupe/memo/efeitos continuam no
//! chamador.

use std::time::Duration;

use serde_json::Value;

use crate::alldebrid::AllDebrid;
use crate::debridlink::DebridLink;
use crate::error::DebridError;
use crate::http::HttpClient;
use crate::realdebrid::RealDebrid;
use crate::torbox::TorBox;

use super::InventoryItem;

pub(super) async fn read_items(
    service: &str,
    client: &HttpClient,
    api_key: &str,
    timeout: Duration,
) -> Result<Vec<InventoryItem>, DebridError> {
    match service {
        "alldebrid" => alldebrid_items(client, api_key, timeout).await,
        "torbox" => torbox_items(client, api_key, timeout).await,
        "realdebrid" => realdebrid_items(client, api_key).await,
        "debridlink" => debridlink_items(client, api_key).await,
        _ => Ok(Vec::new()),
    }
}

// ── AllDebrid: `/magnet/status` ────────────────────────────────────────────
// Mesmo critério do `account_status`: `ready` OU status Ready (case-insensitive).
async fn alldebrid_items(
    client: &HttpClient,
    api_key: &str,
    timeout: Duration,
) -> Result<Vec<InventoryItem>, DebridError> {
    let adapter = AllDebrid::new(client.clone());
    let data = adapter
        .call(api_key, "/magnet/status", &[], timeout, false)
        .await?;
    let rows = data
        .get("magnets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut out = Vec::new();
    for magnet in rows {
        let status = magnet.get("status").and_then(Value::as_str).unwrap_or("");
        let ready = js_truthy(magnet.get("ready")) || status.eq_ignore_ascii_case("ready");
        if !ready {
            continue;
        }
        push_item(
            &mut out,
            text_of(magnet.get("hash")),
            text_of(magnet.get("filename")),
            js_number_or_zero(magnet.get("size")),
            None,
        );
    }
    Ok(out)
}

// ── TorBox: `/torrents/mylist` ─────────────────────────────────────────────
// `download_finished || download_present`, o mesmo par do `resolveLink`.
async fn torbox_items(
    client: &HttpClient,
    api_key: &str,
    timeout: Duration,
) -> Result<Vec<InventoryItem>, DebridError> {
    let adapter = TorBox::new(client.clone());
    let list = adapter
        .call(
            api_key,
            "/torrents/mylist",
            reqwest::Method::GET,
            &[],
            None,
            timeout,
        )
        .await?;
    let rows: &[Value] = match list.get("data") {
        Some(Value::Array(rows)) => rows,
        Some(other) if js_truthy(Some(other)) => std::slice::from_ref(other),
        _ => &[],
    };
    let mut out = Vec::new();
    for row in rows {
        if !(js_truthy(row.get("download_finished")) || js_truthy(row.get("download_present"))) {
            continue;
        }
        let before = out.len();
        push_item(
            &mut out,
            text_of(row.get("hash")),
            text_of(row.get("name")),
            js_number_or_zero(row.get("size")),
            None,
        );
        set_main_file(&mut out, before, row.get("files"));
    }
    Ok(out)
}

// ── Real-Debrid: `/torrents` paginado ──────────────────────────────────────
// `status === 'downloaded'` (comparação EXATA, como o `READY` do TS). O id
// viaja junto: é o que o play usa para resolver pelo memo sem re-listar.
async fn realdebrid_items(
    client: &HttpClient,
    api_key: &str,
) -> Result<Vec<InventoryItem>, DebridError> {
    let rows = RealDebrid::new(client.clone())
        .list_torrents(api_key)
        .await?;
    let mut out = Vec::new();
    for row in &rows {
        if row.get("status").and_then(Value::as_str) != Some("downloaded") {
            continue;
        }
        push_item(
            &mut out,
            text_of(row.get("hash")),
            text_of(row.get("filename")),
            js_number_or_zero(row.get("bytes")),
            text_of(row.get("id")),
        );
    }
    Ok(out)
}

// ── Debrid-Link: `/seedbox/list` ───────────────────────────────────────────
// O TS compara `Number(row.downloadPercent) < 100`; `undefined` vira NaN e
// `NaN < 100` é FALSO — linha sem percentual NÃO é descartada. Reproduzido.
async fn debridlink_items(
    client: &HttpClient,
    api_key: &str,
) -> Result<Vec<InventoryItem>, DebridError> {
    let list = DebridLink::new(client.clone())
        .call(api_key, "/seedbox/list", reqwest::Method::GET, None, &[])
        .await?;
    let rows = list.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut out = Vec::new();
    for row in rows {
        if js_number(row.get("downloadPercent")).is_some_and(|percent| percent < 100.0) {
            continue;
        }
        let hash = text_of(row.get("hash")).or_else(|| text_of(row.get("infoHash")));
        // `Number(row.totalSize || row.size) || 0`: valor truthy vence; zero
        // cai no próximo (o `||` do JS).
        let total = js_number_or_zero(row.get("totalSize"));
        let size = if total != 0.0 {
            total
        } else {
            js_number_or_zero(row.get("size"))
        };
        let before = out.len();
        push_item(&mut out, hash, text_of(row.get("name")), size, None);
        set_main_file(&mut out, before, row.get("files"));
    }
    Ok(out)
}

/// Mesmo filtro de forma dos adaptadores: hash/título vazios e título que é o
/// próprio hash (magnet sem metadado) ficam de fora.
fn push_item(
    out: &mut Vec<InventoryItem>,
    hash: Option<String>,
    title: Option<String>,
    size: f64,
    id: Option<String>,
) {
    let Some(info_hash) = hash
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
    else {
        return;
    };
    let Some(title) = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    else {
        return;
    };
    if title.to_lowercase() == info_hash {
        return;
    }
    out.push(InventoryItem {
        title,
        info_hash,
        size,
        id,
        main_file: None,
    });
}

/// Grava o maior vídeo (sem sample) no item recém-empurrado. `short_name`
/// (TorBox) ou `name`; tamanho em `size`. Sem lista, fica `None`.
fn set_main_file(out: &mut [InventoryItem], before: usize, files: Option<&Value>) {
    if out.len() <= before {
        return;
    }
    let Some(list) = files.and_then(Value::as_array) else {
        return;
    };
    let main = list
        .iter()
        .filter_map(|file| {
            let name = text_of(file.get("short_name")).or_else(|| text_of(file.get("name")))?;
            let video = adom_core::is_video(&name) && !adom_core::is_sample(&name);
            video.then(|| (js_number_or_zero(file.get("size")), name))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, name)| adom_core::base_name(&name));
    if let Some(item) = out.last_mut() {
        item.main_file = main;
    }
}

/// `Value` → string não vazia, com trim? O TS faz `String(x || '')` sem trim
/// e aplica `.trim()` só no título. Aqui o trim é do chamador; este helper só
/// evita `null`/`undefined`.
fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// `Number(value) || 0` do JS (inclui string numérica).
fn js_number_or_zero(value: Option<&Value>) -> f64 {
    js_number(value).unwrap_or(0.0)
}

/// `Number(value)`: `None` é o NaN do JS (e o chamador decide).
fn js_number(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Null => return None,
        Value::Bool(flag) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse().ok()?
            }
        }
        _ => return None,
    };
    number.is_finite().then_some(number)
}

/// Truthiness do JS (o `x || y` dos adaptadores).
fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod main_file_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maior_video_sem_sample_vira_main_file() {
        let mut out = vec![InventoryItem::default()];
        let files = json!([
            {"short_name": "A Noite dos Mortos Vivos (1990) BDrip 1080p Dublado dat2014.mp4", "size": 1_200_000_000u64},
            {"short_name": "A Noite dos Mortos Vivos (1990) BDrip 1080p Dublado dat2014.srt", "size": 90_000},
            {"short_name": "sample.mkv", "size": 1_900_000_000u64},
            {"short_name": "LEIA -ME.txt", "size": 100}
        ]);
        set_main_file(&mut out, 0, Some(&files));
        assert_eq!(
            out[0].main_file.as_deref(),
            Some("A Noite dos Mortos Vivos (1990) BDrip 1080p Dublado dat2014.mp4")
        );
    }

    #[test]
    fn item_descartado_nao_recebe_arquivo_de_outro() {
        let mut out = vec![InventoryItem::default()];
        set_main_file(&mut out, 1, Some(&json!([{"name": "x.mkv", "size": 1}])));
        assert_eq!(out[0].main_file, None);
    }
}
