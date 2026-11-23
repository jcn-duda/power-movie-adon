//! Porte de `explicitPtAudio` e `hasExplicitForeignAudio`
//! (`src/utils/audio-quality.ts`): a prova POSITIVA de áudio PT no título e a
//! lista MÍNIMA de idiomas que autoriza a condenação no caminho destrutivo.
//!
//! `explicit_pt_audio` reusa `generic_dub_proves_pt` (já portado em
//! `audio_lang`): o `DUB`/`DUBBED` genérico só prova PT quando o título não
//! nomeia idioma estrangeiro, não está em cirílico e não carrega assinatura
//! transliterada. Marca PT explícita continua absolvendo pelas alternativas
//! próprias, fora do predicado genérico.
//!
//! `has_explicit_foreign_audio` é OUTRA coisa da guarda ampla
//! `names_foreign_dub_language`: aqui é prova positiva de idioma estrangeiro,
//! a MENOR das duas listas, e é ela que autoriza apagar da conta. A assimetria
//! (guarda ampla generosa × condenação mínima) é deliberada e travada pelo TS;
//! PT explícito absolve antes da lista. NÃO uniformize as duas.
//!
//! Regras de paridade com o JS (sem a flag `u`):
//! - `\b` do JS é ASCII: vira `(?-u:\b)`, como em `audio_lang`/`matching_vocabulary`.
//! - `\s` vira `JS_SPACE` (espaços do ECMAScript, com o BOM U+FEFF).
//! - `toUpperCase` é `str::to_uppercase` (mesmo mapeamento full do Node).
//! - `DUBLAD[OA]S?` sai do `\b` e usa lookaround: dígito colado NÃO desfaz, letra
//!   A-Z colada desfaz. Lookbehind/lookahead exigem `fancy-regex`.

use std::sync::OnceLock;

use regex::Regex;

use crate::audio_lang::generic_dub_proves_pt;
use crate::matching_vocabulary::JS_SPACE;

/// `\b` do JS sem flag `u`: fronteira ASCII, não a fronteira Unicode do crate.
fn bounded(alts: &str) -> String {
    format!(r"(?-u:\b)(?:{alts})(?-u:\b)")
}

/// `LEGENDAD[OA]|LEGENDAS?|LEG[-.]?PT[-.]?BR|SUB[-.]?PT[-.]?BR|SOFT[- ]?SUB`.
fn sub_mark_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&bounded(
            r"LEGENDAD[OA]|LEGENDAS?|LEG[-.]?PT[-.]?BR|SUB[-.]?PT[-.]?BR|SOFT[- ]?SUB",
        ))
        .expect("marcador de legenda compila")
    })
}

/// `\[\s*LEG\s*\]|\(\s*LEG\s*\)|\bLEG\b`, com `\s` do JS e `\b` ASCII.
fn leg_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = format!(
            r"\[{s}*LEG{s}*\]|\({s}*LEG{s}*\)|{word}",
            s = JS_SPACE,
            word = bounded("LEG"),
        );
        Regex::new(&pattern).expect("marcador LEG compila")
    })
}

/// `(?<![A-Z])DUBLAD[OA]S?(?![A-Z])`: dígito colado passa, letra A-Z colada veta.
///
/// Scan manual em vez de `fancy_regex`: o lookaround do `fancy_regex` custa
/// ~1,3 µs por chamada (medido em `tmp/g3-decomp.mjs`, 1203 títulos) e esta é a
/// sonda mais quente do áudio — `explicit_pt_audio` é chamada 2–3× por título
/// (`audio_from_title` e `has_explicit_foreign_audio`). O scan percorre bytes
/// ASCII e trata qualquer byte ≥ 0x80 como "não é A-Z", que é exatamente o que
/// o lookbehind/lookahead do JS vê num code point não-ASCII.
fn dublado_hit(t: &str) -> bool {
    const NEEDLE: &[u8] = b"DUBLAD";
    let bytes = t.as_bytes();
    let mut from = 0usize;
    while let Some(found) = bytes[from..]
        .windows(NEEDLE.len())
        .position(|w| w == NEEDLE)
    {
        let start = from + found;
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_uppercase();
        let mut end = start + NEEDLE.len();
        if matches!(bytes.get(end), Some(b'O') | Some(b'A')) {
            end += 1;
            if bytes.get(end) == Some(&b'S') {
                end += 1;
            }
            let after_ok = bytes.get(end).is_none_or(|b| !b.is_ascii_uppercase());
            if before_ok && after_ok {
                return true;
            }
        }
        from = start + 1;
    }
    false
}

/// `\b(DUBLAGEM|DUB[-.]?BR|AUDIO[- ]?PT[-.]?BR)\b`.
fn dubagem_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&bounded(r"DUBLAGEM|DUB[-.]?BR|AUDIO[- ]?PT[-.]?BR"))
            .expect("marcador de dublagem compila")
    })
}

/// `\b(PT[-.]?BR|PTBR|PORTUGU[EÊ]S|BRAZILIAN)\b` (o `Ê` é U+00CA).
fn pt_br_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&bounded(r"PT[-.]?BR|PTBR|PORTUGU[E\x{CA}]S|BRAZILIAN"))
            .expect("marcador PT-BR compila")
    })
}

/// `\b(TRUEFRENCH|FRENCH|VOSTFR|VF|ITA|SUBITA|ESPANOL|CASTELLANO|RUS|GERMAN|NL|HINDI|UKR|UKRAINIAN)\b`.
///
/// A lista MÍNIMA (14 idiomas): só prova positiva de idioma estrangeiro entra,
/// um por vez, e só depois de medição real. `ENG`/`ENGLISH`/`SPA`/`POLISH`… que
/// a guarda ampla aceita NÃO entram aqui.
fn foreign_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&bounded(
            r"TRUEFRENCH|FRENCH|VOSTFR|VF|ITA|SUBITA|ESPANOL|CASTELLANO|RUS|GERMAN|NL|HINDI|UKR|UKRAINIAN",
        ))
        .expect("lista mínima de áudio estrangeiro compila")
    })
}

/// `explicitPtAudio`: prova POSITIVA de áudio PT no título da release.
pub fn explicit_pt_audio(title: &str) -> bool {
    // `_` separa de fato: sem a troca o `\b` ASCII não vê fronteira e o
    // marcador colado ao underscore não casava.
    let t = title.to_uppercase().replace('_', " ");
    let is_explicit_sub = sub_mark_re().is_match(&t) || leg_re().is_match(&t);
    let is_generic_dub = generic_dub_proves_pt(&t);

    dublado_hit(&t)
        || dubagem_re().is_match(&t)
        || is_generic_dub
        // `PT-BR` ao lado de uma marca de legenda NÃO é áudio PT: o botão é
        // legendado. Sem a guarda, `FOO LEGENDADO PT-BR` viraria dublado. Idem
        // para `PTBR` em lista de idiomas ("En/PTBR") ou com SUB/HARDSUB.
        || (pt_br_re().is_match(&t) && !is_explicit_sub && !bare_pt_br_is_subtitle_list(&t))
}

/// Idiomas que, colados ao `PTBR`, formam LISTA de idiomas ("En/PTBR",
/// "[PTBR ENG]", "[ENG FR PTBR]").
const LANG_CODES: &str =
    "EN|ENG|ENGLISH|ING|INGLES|FR|FRE|FRA|ES|ESP|SPA|JP|JPN|JAP|IT|DE|GER|RU|KOR|CHI";

/// `PTBR` LISTADO ao lado de outro idioma: em tracker global é quase sempre
/// lista de LEGENDA. Caso medido: "The Fix 2026 1080p WEBSCR HEVC x265 En/PTBR"
/// (EXT, mesmos 1.95 GB da release BONE inglesa) chegava BR com vaga reservada
/// no topo e o áudio era só inglês. Na varredura do Jackett o padrão "idioma +
/// PTBR" apareceu só em fansub/legenda (Darkanimes "[ENG FR PTBR]", "leg-PTBR").
fn pt_lang_list_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let sep = format!(r"[{JS_SPACE}/+&,|.\[\]-]*");
        let pt = bounded(r"PT[-.]?BR");
        let lang = bounded(LANG_CODES);
        Regex::new(&format!("{lang}{sep}{pt}|{pt}{sep}{lang}"))
            .expect("lista de idiomas com PTBR compila")
    })
}

/// Marca de LEGENDA genérica: só desqualifica o `PTBR` nu, nunca DUBLADO/DUAL.
fn bare_sub_mark_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&bounded(r"HARD[-.]?SUBS?|SUBS?|SUBTITLES?|SUBBED"))
            .expect("marca de legenda genérica compila")
    })
}

/// Com marca de ÁUDIO explícita ("DUAL En/PTBR") a lista volta a descrever
/// faixas — o PT nu segue valendo como antes.
fn audio_mark_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&bounded(r"DUAL|MULTI|AUDIO|DUB|DUBBED")).expect("marca de áudio compila")
    })
}

/// `PTBR` nu num título que o lista entre idiomas ou declara legenda: descreve
/// legenda, não áudio. Recebe o título já em CAIXA ALTA.
fn bare_pt_br_is_subtitle_list(t: &str) -> bool {
    !audio_mark_re().is_match(t)
        && (pt_lang_list_re().is_match(t) || bare_sub_mark_re().is_match(t))
}

/// `hasExplicitForeignAudio`: lista MÍNIMA dos caminhos destrutivos.
///
/// PT explícito absolve antes: um único marcador PT em qualquer lugar vence a
/// lista — falso positivo aqui destrói acervo BR da conta.
pub fn has_explicit_foreign_audio(title: &str) -> bool {
    if explicit_pt_audio(title) {
        return false;
    }
    foreign_re().is_match(&title.to_uppercase())
}

#[cfg(test)]
mod lista_de_idiomas_tests {
    use super::explicit_pt_audio;

    // Caso The Fix 2026 e a varredura do Jackett: PTBR listado entre idiomas
    // ou com marca de legenda é legenda, não áudio.
    #[test]
    fn ptbr_em_lista_de_idiomas_nao_prova_audio() {
        for t in [
            "The Fix 2026 1080p WEBSCR HEVC x265 En/PTBR",
            "[Darkanimes] Dorohedoro  04 [1080P x265 10bit][PTBR ENG] mkv",
            "[Darkanimes] Dorohedoro  01 [1080p][HEVC x265 10bit][ENG FR PTBR] mkv",
            "[ViperFansub] Dragon Quest  Dai no Daibouken  03 (1080p) x265 [Hardsub]PTBR",
            "[J-SONGS] Super Hero Spirits 2025 [PTBR-ENG]",
        ] {
            assert!(!explicit_pt_audio(t), "{t}");
        }
    }

    #[test]
    fn marca_de_audio_mantem_ptbr_como_prova() {
        for t in [
            "Camp (2023) 1080p WEB-DL x264 DUAL PT-BR",
            "Learning You (2026) 1080p WEB-DL DUBLADO PT-BR",
            "Maleficent 2014 2160p BluRay REMUX HEVC HDR DUAL PTBR BrRemux",
            "Goldfinger.1964.2160p.BluRay.x265.10bit.DTS.6CH.DUAL-AUDIO.PT-BR-TRB",
            "Harry.Potter.2004.4K.UHD.2160p.REMUX.DV.DTS-HD.MA.7.1.Dual.PTBR-BrRemux",
            "Parent.Trap.2.1986.PTBR.1080P",
            "Filme 2020 1080p DUAL En/PTBR",
        ] {
            assert!(explicit_pt_audio(t), "{t}");
        }
    }
}
