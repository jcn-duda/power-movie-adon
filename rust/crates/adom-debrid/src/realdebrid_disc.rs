//! DVD (`VIDEO_TS`) no play do Real-Debrid. A escolha é ANTES do download:
//! com `NoVideo` e lista de VOBs, o `selectFiles` leva os ids das partes do
//! título principal. Pronto, `links` segue a ordem dos arquivos selecionados,
//! e cada parte é desbloqueada (`/unrestrict/link`). O play devolve as partes
//! em ordem para o app Power Movie emendar; nada de `NoVideo` + `bad` de 24h.
//! Kill-switch: `DEBRID_DVD_PARTS=false`.

use adom_core::{base_name, dvd_title_parts, PickCode};
use reqwest::Method;
use serde_json::Value;

use super::{as_debrid, as_debrid_ref, download_of, form_encode, link_at, RdFile, RealDebrid};
use crate::error::DebridError;
use crate::play::{dvd_parts_enabled, DiscPart, PlayLink};

impl RealDebrid {
    /// Contrato de link único (`/debrid/link` e autofetch): DVD não cabe nele
    /// e continua `NoVideo`.
    pub async fn resolve_link(
        &self,
        api_key: &str,
        hash: &str,
        hint: &adom_core::PlayHint,
        magnet: Option<&str>,
        poll_wait: std::time::Duration,
    ) -> Result<Option<String>, DebridError> {
        match self
            .resolve_play(api_key, hash, hint, magnet, poll_wait)
            .await?
        {
            PlayLink::Url(url) => Ok(url),
            PlayLink::Disc(_) => Err(DebridError::from_pick(PickCode::NoVideo, None, None)),
        }
    }
}

/// `files=` do `selectFiles` para um DVD: ids das partes, separados por vírgula.
pub(super) fn select_param(files: &[RdFile], code: PickCode) -> Option<String> {
    if code != PickCode::NoVideo || !dvd_parts_enabled() {
        return None;
    }
    let parts = dvd_title_parts(&as_debrid(files))?;
    let ids: Option<Vec<String>> = parts
        .iter()
        .map(|part| {
            files
                .iter()
                .find(|file| file.path == part.path && (file.size - part.size).abs() < 0.5)
                .and_then(|file| file.id.clone())
        })
        .collect();
    Some(ids?.join(","))
}

/// Torrent pronto cujos SELECIONADOS formam um DVD: uma URL desbloqueada por
/// parte. Parte sem link derruba o conjunto (filme com buraco é pior).
pub(super) async fn finish(
    rd: &RealDebrid,
    api_key: &str,
    info: &Value,
    selected: &[&RdFile],
) -> Result<Option<Vec<DiscPart>>, DebridError> {
    if !dvd_parts_enabled() {
        return Ok(None);
    }
    let view = as_debrid_ref(selected);
    if view.iter().any(|file| adom_core::is_video(&file.path)) {
        return Ok(None);
    }
    let Some(parts) = dvd_title_parts(&view) else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let Some(idx) = selected
            .iter()
            .position(|file| file.path == part.path && (file.size - part.size).abs() < 0.5)
        else {
            return Ok(None);
        };
        let Some(link) = link_at(info, idx) else {
            return Ok(None);
        };
        let body = format!("link={}", form_encode(&link)).into_bytes();
        let data = rd
            .call(api_key, Method::POST, "/unrestrict/link", Some(body))
            .await?;
        let Some(url) = download_of(data.as_ref()) else {
            return Ok(None);
        };
        out.push(DiscPart {
            url,
            size: part.size,
            name: base_name(&part.path),
        });
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, path: &str, size: f64) -> RdFile {
        RdFile {
            id: Some(id.into()),
            path: path.into(),
            size,
            selected: false,
        }
    }

    #[test]
    fn select_files_leva_os_ids_das_partes_em_ordem() {
        let gb = 1_073_741_824.0;
        let files = vec![
            file("1", "/VIDEO_TS/VIDEO_TS.IFO", 12_000.0),
            file("2", "/VIDEO_TS/VTS_01_0.VOB", 40_000_000.0),
            file("4", "/VIDEO_TS/VTS_01_2.VOB", gb),
            file("3", "/VIDEO_TS/VTS_01_1.VOB", gb),
        ];
        assert_eq!(
            select_param(&files, PickCode::NoVideo).as_deref(),
            Some("3,4")
        );
        assert_eq!(select_param(&files, PickCode::EpisodePick), None);
    }
}
