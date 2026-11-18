//! Escolha do arquivo no play do Debrid-Link (`/seedbox/list` → `downloadUrl`).
//!
//! DVD (`VIDEO_TS`): com `NoVideo` e lista de VOBs, o `downloadUrl` de cada
//! parte do título principal já vem na mesma lista — sem chamada extra. O play
//! devolve as partes em ordem para o app Power Movie emendar no player.
//! Kill-switch: `DEBRID_DVD_PARTS=false`.

use super::*;
use crate::play::DiscPart;
use adom_core::{base_name, dvd_title_parts};

impl DebridLink {
    /// Contrato de link único (`/debrid/link`): DVD não cabe nele e continua
    /// `NoVideo`.
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
            PlayLink::Url(url) => Ok(url),
            PlayLink::Disc(_) => Err(DebridError::from_pick(PickCode::NoVideo, None, None)),
        }
    }
}

pub(super) fn link_for_entry(entry: &Value, hint: &PlayHint) -> Result<PlayLink, DebridError> {
    let remote = remote_files(entry);
    let files: Vec<DebridFile> = remote
        .iter()
        .map(|file| DebridFile {
            path: file.path.clone(),
            size: file.size,
        })
        .collect();
    match pick_file(&files, hint) {
        PickOutcome::Empty => {
            // O TS audita mesmo sem arquivo escolhido (lista vazia não condena).
            crate::play_audit::assert_dubbed(&files, hint)?;
            Ok(PlayLink::Url(None))
        }
        PickOutcome::File { path, size } => {
            crate::play_audit::assert_dubbed(&files, hint)?;
            Ok(PlayLink::Url(download_link(&remote, &path, size)))
        }
        PickOutcome::Error {
            code,
            evidence,
            context,
        } => match disc_parts(&remote, &files, code) {
            Some(parts) => Ok(PlayLink::Disc(parts)),
            None => Err(pick_error(code, evidence, context)),
        },
    }
}

fn download_link(files: &[RemoteFile], path: &str, size: f64) -> Option<String> {
    let mut fallback = None;
    for file in files {
        if file.path != path {
            continue;
        }
        if (file.size - size).abs() < 0.5 {
            return file.link.clone();
        }
        if fallback.is_none() {
            fallback = file.link.clone();
        }
    }
    fallback
}

/// Partes do DVD com os `downloadUrl` da lista. Parte sem link derruba o
/// conjunto: filme com buraco no meio é pior que a recusa.
fn disc_parts(
    remote: &[RemoteFile],
    files: &[DebridFile],
    code: PickCode,
) -> Option<Vec<DiscPart>> {
    if code != PickCode::NoVideo || !crate::play::dvd_parts_enabled() {
        return None;
    }
    dvd_title_parts(files)?
        .into_iter()
        .map(|part| {
            Some(DiscPart {
                url: download_link(remote, &part.path, part.size)?,
                size: part.size,
                name: base_name(&part.path),
            })
        })
        .collect()
}

#[cfg(test)]
mod disc_tests {
    use super::*;

    #[test]
    fn dvd_vira_partes_com_os_download_urls() {
        let gb = 1_073_741_824.0;
        let remote = vec![
            RemoteFile {
                path: "VIDEO_TS/VTS_01_2.VOB".into(),
                size: gb,
                link: Some("https://dl/2".into()),
            },
            RemoteFile {
                path: "VIDEO_TS/VTS_01_1.VOB".into(),
                size: gb,
                link: Some("https://dl/1".into()),
            },
        ];
        let files: Vec<DebridFile> = remote
            .iter()
            .map(|file| DebridFile {
                path: file.path.clone(),
                size: file.size,
            })
            .collect();
        let parts = disc_parts(&remote, &files, PickCode::NoVideo).expect("dvd");
        let urls: Vec<&str> = parts.iter().map(|part| part.url.as_str()).collect();
        assert_eq!(urls, ["https://dl/1", "https://dl/2"]);
    }
}
