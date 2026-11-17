//! DVD (`VIDEO_TS`) no play do TorBox: o filme são N `VTS_xx_k.VOB`, não um
//! arquivo. Em vez de `NoVideo` (que grava `bad` por 24h), o play devolve um
//! link direto do TorBox POR PARTE, em ordem; o app Power Movie emenda as
//! partes no player (Media3 `ConcatenatingMediaSource2` / mpv `edl://`). Nenhum
//! byte de vídeo passa pelo servidor.
//!
//! Kill-switch: `DEBRID_DVD_PARTS=false` volta ao `NoVideo` de antes.

use adom_core::{
    base_name, dvd_title_parts, pick_file, DebridFile, PickCode, PickOutcome, PlayHint,
};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

use super::torbox_check::{self, RemoteFile};
use super::{TorBox, CALL_TIMEOUT};
use crate::error::DebridError;
use crate::play::DiscPart;

/// Resultado do play do TorBox: link único (ou nada, ainda baixando) ou as
/// partes de um DVD.
#[derive(Debug, Clone, PartialEq)]
pub enum TorboxLink {
    Url(Option<String>),
    Disc(Vec<DiscPart>),
}

impl TorBox {
    /// Contrato antigo (rota `/debrid/link`): um link só. DVD não cabe nele e
    /// continua sendo `NoVideo`.
    pub async fn resolve_link(
        &self,
        api_key: &str,
        hash: &str,
        hint: &PlayHint,
        magnet: Option<&str>,
        poll_wait: Duration,
    ) -> Result<Option<String>, DebridError> {
        match self
            .resolve_play(api_key, hash, hint, magnet, poll_wait)
            .await?
        {
            TorboxLink::Url(url) => Ok(url),
            TorboxLink::Disc(_) => Err(DebridError::from_pick(PickCode::NoVideo, None, None)),
        }
    }

    pub(super) async fn link_for(
        &self,
        api_key: &str,
        torrent_id: &str,
        entry: &Value,
        hint: &PlayHint,
    ) -> Result<TorboxLink, DebridError> {
        let remote = torbox_check::remote_files(entry);
        let files: Vec<DebridFile> = remote
            .iter()
            .map(|file| DebridFile {
                path: file.path.clone(),
                size: file.size,
            })
            .collect();
        let (path, size) = match pick_file(&files, hint) {
            PickOutcome::Empty => {
                // O TS audita mesmo sem arquivo escolhido (lista vazia não
                // condena, mas a ordem é a dele).
                crate::play_audit::assert_dubbed(&files, hint)?;
                return Ok(TorboxLink::Url(None));
            }
            PickOutcome::Error {
                code,
                evidence,
                context,
            } => {
                let disc = self.disc_parts(api_key, torrent_id, &remote, &files, code);
                if let Some(parts) = disc.await? {
                    return Ok(TorboxLink::Disc(parts));
                }
                return Err(torbox_check::pick_error(code, evidence, context));
            }
            PickOutcome::File { path, size } => (path, size),
        };
        crate::play_audit::assert_dubbed(&files, hint)?;
        let file_id = torbox_check::chosen_id(&remote, &path, size);
        let dl = self
            .call(
                api_key,
                "/torrents/requestdl",
                Method::GET,
                &[
                    ("token", api_key),
                    ("torrent_id", torrent_id),
                    ("file_id", file_id.as_str()),
                    ("redirect", "false"),
                ],
                None,
                CALL_TIMEOUT,
            )
            .await?;
        Ok(TorboxLink::Url(torbox_check::download_url(&dl)))
    }

    /// Partes do título principal quando o seletor recusou por `NoVideo` e a
    /// lista é um DVD. Qualquer parte sem id/link derruba o conjunto inteiro:
    /// filme com buraco no meio é pior que a recusa.
    pub(super) async fn disc_parts(
        &self,
        api_key: &str,
        torrent_id: &str,
        remote: &[RemoteFile],
        files: &[DebridFile],
        code: PickCode,
    ) -> Result<Option<Vec<DiscPart>>, DebridError> {
        if code != PickCode::NoVideo || !dvd_parts_enabled() {
            return Ok(None);
        }
        let Some(parts) = dvd_title_parts(files) else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            let file_id = torbox_check::chosen_id(remote, &part.path, part.size);
            if file_id.is_empty() {
                return Ok(None);
            }
            let dl = self
                .call(
                    api_key,
                    "/torrents/requestdl",
                    Method::GET,
                    &[
                        ("token", api_key),
                        ("torrent_id", torrent_id),
                        ("file_id", file_id.as_str()),
                        ("redirect", "false"),
                    ],
                    None,
                    CALL_TIMEOUT,
                )
                .await?;
            let Some(url) = torbox_check::download_url(&dl) else {
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
}

/// Ligado por padrão: sem isto o DVD falha do mesmo jeito (`NoVideo`).
fn dvd_parts_enabled() -> bool {
    !matches!(
        std::env::var("DEBRID_DVD_PARTS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "off"
    )
}
