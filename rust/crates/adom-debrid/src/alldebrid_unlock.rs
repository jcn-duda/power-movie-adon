//! Desbloqueio do arquivo escolhido no play da AllDebrid (`/link/unlock`).
//!
//! DVD (`VIDEO_TS`): com `NoVideo` e lista de VOBs, cada parte do título
//! principal é desbloqueada — um `/link/unlock` por VOB — e o play devolve as
//! partes em ordem para o app Power Movie emendar no player, em vez do
//! `NoVideo` que gravava `bad` por 24h. Kill-switch: `DEBRID_DVD_PARTS=false`.

use super::*;
use adom_core::{base_name, dvd_title_parts, PickCode};

impl AllDebrid {
    pub(super) async fn unlock_picked(
        &self,
        api_key: &str,
        magnet: &Value,
        hint: &PlayHint,
    ) -> Result<PlayResult, PlayError> {
        let mut leaves = Vec::new();
        if let Some(files) = magnet.get("files").and_then(Value::as_array) {
            flatten(files, "", &mut leaves);
        }
        let picked = pick_file(&as_debrid(&leaves), hint);
        // Ordem do TS: `pickFile` lança antes da prova (sem vídeo, episódio);
        // depois dele a lista é gravada mesmo se a dublagem mentir.
        let files = Some(play_files(&leaves));
        match picked {
            PickOutcome::Empty => Ok(PlayResult {
                files,
                ..PlayResult::default()
            }),
            PickOutcome::Error {
                code,
                evidence,
                context,
            } => match self.disc_parts(api_key, &leaves, code).await? {
                Some(parts) => Ok(PlayResult {
                    files,
                    parts: Some(parts),
                    ..PlayResult::default()
                }),
                None => Err(pick_error(code, evidence, context).into()),
            },
            PickOutcome::File { path, size } => {
                // A prova é o caminho do arquivo. Mentira sai antes do
                // unlock: não abre o link, não apaga o torrent, não marca.
                if let Err(error) = play_audit::assert_dubbed(&as_debrid(&leaves), hint) {
                    return Err(PlayError { error, files });
                }
                let Some(link) = leaf_link(&leaves, &path, size) else {
                    return Ok(PlayResult {
                        files,
                        ..PlayResult::default()
                    });
                };
                let unlocked = self
                    .call(
                        api_key,
                        "/link/unlock",
                        &[("link", &link)],
                        CALL_TIMEOUT,
                        false,
                    )
                    .await?;
                Ok(PlayResult {
                    url: nonempty(unlocked.get("link")),
                    files,
                    ..PlayResult::default()
                })
            }
        }
    }

    /// Partes do DVD desbloqueadas, em ordem. Parte sem link derruba o
    /// conjunto: filme com buraco no meio é pior que a recusa.
    async fn disc_parts(
        &self,
        api_key: &str,
        leaves: &[Leaf],
        code: PickCode,
    ) -> Result<Option<Vec<crate::play::DiscPart>>, DebridError> {
        if code != PickCode::NoVideo || !crate::play::dvd_parts_enabled() {
            return Ok(None);
        }
        let Some(parts) = dvd_title_parts(&as_debrid(leaves)) else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            let Some(link) = leaf_link(leaves, &part.path, part.size) else {
                return Ok(None);
            };
            let unlocked = self
                .call(
                    api_key,
                    "/link/unlock",
                    &[("link", &link)],
                    CALL_TIMEOUT,
                    false,
                )
                .await?;
            let Some(url) = nonempty(unlocked.get("link")) else {
                return Ok(None);
            };
            out.push(crate::play::DiscPart {
                url,
                size: part.size,
                name: base_name(&part.path),
            });
        }
        Ok(Some(out))
    }
}
