//! DVD (`VIDEO_TS`) no play do Premiumize. O `/transfer/directdl` já devolve
//! o link de CADA arquivo, então as partes do título principal saem da mesma
//! resposta — sem chamada extra. Mesmo contrato do TorBox (`torbox_disc.rs`):
//! em vez de `NoVideo` (que grava `bad` por 24h), as partes em ordem para o
//! app Power Movie emendar no player.

use adom_core::{base_name, dvd_title_parts, pick_file, PickCode, PickOutcome, PlayHint};

use super::{as_debrid, ContentFile, Premiumize, CALL_TIMEOUT};
use crate::error::DebridError;
use crate::play::{DiscPart, PlayLink};

impl Premiumize {
    /// Play com suporte a DVD. Fora do caso DVD é exatamente o `resolve_link`.
    pub async fn resolve_play(
        &self,
        api_key: &str,
        hash: &str,
        hint: &PlayHint,
        magnet: Option<&str>,
    ) -> Result<PlayLink, DebridError> {
        let files = self.directdl(api_key, hash, magnet, CALL_TIMEOUT).await?;
        if let Some(parts) = disc_parts(&files, hint) {
            return Ok(PlayLink::Disc(parts));
        }
        self.link_from(files, hint).map(PlayLink::Url)
    }
}

/// Partes do DVD só quando o seletor recusa por `NoVideo`. Parte sem link
/// derruba o conjunto: filme com buraco no meio é pior que a recusa.
fn disc_parts(files: &[ContentFile], hint: &PlayHint) -> Option<Vec<DiscPart>> {
    if !crate::play::dvd_parts_enabled() {
        return None;
    }
    let view = as_debrid(files);
    let PickOutcome::Error {
        code: PickCode::NoVideo,
        ..
    } = pick_file(&view, hint)
    else {
        return None;
    };
    dvd_title_parts(&view)?
        .into_iter()
        .map(|part| {
            let file = files
                .iter()
                .find(|file| file.path == part.path && !file.link.is_empty())?;
            Some(DiscPart {
                url: file.link.clone(),
                size: part.size,
                name: base_name(&part.path),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, link: &str) -> ContentFile {
        ContentFile {
            path: path.into(),
            size,
            link: link.into(),
        }
    }

    const GB: u64 = 1 << 30;

    #[test]
    fn dvd_vira_partes_com_os_links_do_directdl() {
        let files = vec![
            file("Night Life/VIDEO_TS/VIDEO_TS.IFO", 12_000, "https://pm/ifo"),
            file("Night Life/VIDEO_TS/VTS_01_2.VOB", GB, "https://pm/2"),
            file("Night Life/VIDEO_TS/VTS_01_1.VOB", GB, "https://pm/1"),
            file(
                "Night Life/VIDEO_TS/VTS_01_0.VOB",
                40_000_000,
                "https://pm/0",
            ),
        ];
        let parts = disc_parts(&files, &PlayHint::default()).expect("dvd");
        let urls: Vec<&str> = parts.iter().map(|part| part.url.as_str()).collect();
        assert_eq!(urls, ["https://pm/1", "https://pm/2"]);
        assert_eq!(parts[0].name, "VTS_01_1.VOB");
    }

    #[test]
    fn parte_sem_link_derruba_o_dvd() {
        let files = vec![
            file("VIDEO_TS/VTS_01_1.VOB", GB, "https://pm/1"),
            file("VIDEO_TS/VTS_01_2.VOB", GB, ""),
        ];
        assert_eq!(disc_parts(&files, &PlayHint::default()), None);
    }

    #[test]
    fn torrent_com_video_comum_nao_e_dvd() {
        let files = vec![
            file("Movie.mkv", 2 * GB, "https://pm/mkv"),
            file("extras/VIDEO_TS/VTS_01_1.VOB", GB, "https://pm/1"),
        ];
        assert_eq!(disc_parts(&files, &PlayHint::default()), None);
    }
}
