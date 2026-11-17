//! Estrutura de DVD (`VIDEO_TS`): o filme não é UM arquivo, é uma sequência
//! de `VTS_xx_1..N.VOB` de ~1 GB. O seletor comum devolve `NoVideo` porque
//! `.VOB` não é vídeo tocável sozinho — tocar só o maior pedaço daria ~30 min
//! e pararia. Aqui sai a LISTA ordenada do título principal, para o cliente
//! (o app Power Movie) emendar as partes no próprio player.
//!
//! Sem IO: só decisão sobre a lista que o serviço reportou.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;

use super::DebridFile;

/// `VIDEO_TS/VTS_01_3.VOB` → (título 1, parte 3). O `_0` é o menu do título.
fn vob_re() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[/\\])VTS_(\d{2})_(\d{1,2})\.VOB$").expect("regex de VOB")
    })
}

/// Título principal de um DVD: o `VTS_xx` de maior tamanho somado, partes em
/// ordem. `None` quando não há DVD, quando o título tem buraco na numeração
/// (parte faltando = filme cortado no meio) ou quando a soma não passa de
/// [`MIN_TITLE_BYTES`] (extras/trailer não são o filme).
pub fn dvd_title_parts(files: &[DebridFile]) -> Option<Vec<DebridFile>> {
    let mut titles: BTreeMap<u32, Vec<(u32, &DebridFile)>> = BTreeMap::new();
    for file in files {
        let Some(caps) = vob_re().captures(&file.path) else {
            continue;
        };
        let title: u32 = caps[1].parse().ok()?;
        let part: u32 = caps[2].parse().ok()?;
        if part == 0 {
            continue;
        }
        titles.entry(title).or_default().push((part, file));
    }
    let (_, mut parts) = titles
        .into_iter()
        .filter(|(_, parts)| contiguous(parts))
        .max_by(|(_, a), (_, b)| total(a).total_cmp(&total(b)))?;
    if total(&parts) < MIN_TITLE_BYTES {
        return None;
    }
    parts.sort_by_key(|(part, _)| *part);
    Some(parts.into_iter().map(|(_, file)| file.clone()).collect())
}

/// Título de release que anuncia DISCO (DVD5/DVD9/DVD-R/`VIDEO_TS`/FULL DVD/
/// ISO): a lista marca `[DVD]` e o empurra para o fim. Medido no acervo de
/// produção (2026-11-17): 0,6% dos releases, quase todo Rutracker/cena
/// nórdica. `DVDRip`/`DVDScr` é ENCODE (um arquivo comum) — não é disco, a
/// menos que o mesmo título cite DVD5/DVD9 (`VHSRip (DVD5)`).
pub fn looks_disc_release(title: &str) -> bool {
    static DISC: OnceLock<Regex> = OnceLock::new();
    static STRONG: OnceLock<Regex> = OnceLock::new();
    static RIP: OnceLock<Regex> = OnceLock::new();
    let disc = DISC.get_or_init(|| {
        Regex::new(r"(?i)\bDVD[-_ ]?[59]\b|VIDEO_TS|\bFULL[ ._-]?DVD\b|\bISO\b|\bDVD[-_ ]?R\b")
            .expect("regex de disco")
    });
    if !disc.is_match(title) {
        return false;
    }
    let strong = STRONG.get_or_init(|| {
        Regex::new(r"(?i)\bDVD[-_ ]?[59]\b|VIDEO_TS|\bFULL[ ._-]?DVD\b").expect("regex forte")
    });
    let rip = RIP.get_or_init(|| Regex::new(r"(?i)DVD[-_. ]?(?:RIP|SCR)").expect("regex rip"));
    strong.is_match(title) || !rip.is_match(title)
}

/// Abaixo disso o "maior título" é extra ou menu animado, não o filme: um DVD
/// de longa tem 1 GB ou mais no título principal.
pub const MIN_TITLE_BYTES: f64 = 300.0 * 1024.0 * 1024.0;

fn total(parts: &[(u32, &DebridFile)]) -> f64 {
    parts.iter().map(|(_, file)| file.size.max(0.0)).sum()
}

/// Partes 1..N sem buraco e sem repetição.
fn contiguous(parts: &[(u32, &DebridFile)]) -> bool {
    let mut numbers: Vec<u32> = parts.iter().map(|(part, _)| *part).collect();
    numbers.sort_unstable();
    numbers
        .iter()
        .enumerate()
        .all(|(index, part)| *part == index as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    fn file(path: &str, size: f64) -> DebridFile {
        DebridFile {
            path: path.into(),
            size,
        }
    }

    #[test]
    fn escolhe_o_titulo_principal_em_ordem() {
        let files = vec![
            file("Night Life/VIDEO_TS/VIDEO_TS.IFO", 12_000.0),
            file("Night Life/VIDEO_TS/VTS_01_0.VOB", 50_000_000.0),
            file("Night Life/VIDEO_TS/VTS_01_2.VOB", GB),
            file("Night Life/VIDEO_TS/VTS_01_1.VOB", GB),
            file("Night Life/VIDEO_TS/VTS_01_3.VOB", 0.9 * GB),
            file("Night Life/VIDEO_TS/VTS_02_1.VOB", 0.2 * GB),
            file("Night Life/VIDEO_TS/VTS_01_1.BUP", 30_000.0),
        ];
        let parts = dvd_title_parts(&files).expect("dvd");
        let names: Vec<&str> = parts.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Night Life/VIDEO_TS/VTS_01_1.VOB",
                "Night Life/VIDEO_TS/VTS_01_2.VOB",
                "Night Life/VIDEO_TS/VTS_01_3.VOB",
            ]
        );
    }

    #[test]
    fn caixa_baixa_e_barra_invertida_tambem_valem() {
        let files = vec![
            file(r"DVD\video_ts\vts_03_1.vob", GB),
            file(r"DVD\video_ts\vts_03_2.vob", 0.5 * GB),
        ];
        assert_eq!(dvd_title_parts(&files).map(|p| p.len()), Some(2));
    }

    #[test]
    fn parte_faltando_nao_vira_filme_cortado() {
        let files = vec![
            file("VIDEO_TS/VTS_01_1.VOB", GB),
            file("VIDEO_TS/VTS_01_3.VOB", GB),
        ];
        assert_eq!(dvd_title_parts(&files), None);
    }

    #[test]
    fn so_extras_pequenos_nao_e_filme() {
        let files = vec![file("VIDEO_TS/VTS_01_1.VOB", 80_000_000.0)];
        assert_eq!(dvd_title_parts(&files), None);
    }

    #[test]
    fn titulo_de_disco_x_encode() {
        for disco in [
            "Spawn of Darkness / Nightlife / Night Life [1989, USA, horror, VHSRip (DVD5)] AVO",
            "Night Of The Living Dead 1990 PAL MULTI FULL DVD5 ISO-Tetine",
            "Resident Evil (2002) DVD9 NTSC (WS)[hjsn1]",
            "Resident.Evil.2002.NORDiC.PAL.DVDR-DYSTOPiA",
            "Resident Evil FULL DVD ISO",
        ] {
            assert!(looks_disc_release(disco), "{disco}");
        }
        for encode in [
            "Night.of.the.Living.Dead.1990.DVDRip.XviD",
            "Panico.na.Ilha.2011.DVDRip.Dual",
            "Night Life 1989 mp4",
            "Movie 2020 1080p BluRay x264",
            "Isolation.2005.1080p.WEB-DL",
        ] {
            assert!(!looks_disc_release(encode), "{encode}");
        }
    }

    #[test]
    fn torrent_comum_nao_e_dvd() {
        let files = vec![file("Movie.2020.1080p.mkv", 2.0 * GB)];
        assert_eq!(dvd_title_parts(&files), None);
    }
}
