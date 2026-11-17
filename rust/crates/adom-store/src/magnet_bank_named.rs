//! URI do acervo para os uploads ao debrid, com nome.
//!
//! Item do Torrentio chega só com o hash: o acervo grava o magnet padrão (sem
//! `dn=`) e o título à parte. Subido assim, Premiumize/TorBox mostravam o hash
//! no lugar do nome ("Night Life (1989) Mkv", 2026-11-16). O título do acervo
//! vira o `dn=` quando a URI gravada não tem um.

use rusqlite::{params, OptionalExtension};

use super::super::MagnetBankStore;
use super::err_ctx;

impl MagnetBankStore {
    /// `uri` do hash; sem `dn=` nela, o título do acervo entra como nome.
    pub fn bank_lookup_named_uri(&self, hash: &str) -> Result<Option<String>, String> {
        let row = self
            .lock()
            .query_row(
                "SELECT uri, title FROM magnet WHERE hash = ?1",
                params![hash.to_lowercase()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(err_ctx("lookup nomeado do banco de magnets"))?;
        Ok(row.map(|(uri, title)| with_name(&uri, &title)))
    }
}

/// Acrescenta `&dn=<título>` à URI que não tem nome. URI vazia ou que não é
/// magnet volta como está (quem chama decide o fallback).
pub(crate) fn with_name(uri: &str, title: &str) -> String {
    let title = title.trim();
    let has_name = uri
        .split(['?', '&'])
        .any(|part| part.len() > 3 && part[..3].eq_ignore_ascii_case("dn="));
    if title.is_empty() || has_name || !uri.starts_with("magnet:?") {
        return uri.to_string();
    }
    format!("{uri}&dn={}", encode_component(title))
}

/// `encodeURIComponent`: só o não-reservado passa cru.
fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::with_name;

    const BARE: &str =
        "magnet:?xt=urn:btih:6570156d45ab094d0a63eddbe306a3f58ff50e41&tr=udp%3A%2F%2Ft%3A1";

    #[test]
    fn uri_sem_nome_ganha_o_titulo_do_acervo() {
        assert_eq!(
            with_name(BARE, "Night Life (1989) Mkv"),
            format!("{BARE}&dn=Night%20Life%20(1989)%20Mkv")
        );
        assert_eq!(
            with_name(BARE, "Caça às Bruxas"),
            format!("{BARE}&dn=Ca%C3%A7a%20%C3%A0s%20Bruxas")
        );
    }

    #[test]
    fn uri_com_nome_titulo_vazio_ou_nao_magnet_ficam_como_estao() {
        let named = format!("{BARE}&dn=Original");
        assert_eq!(with_name(&named, "Outro"), named);
        let first = "magnet:?dn=X&xt=urn:btih:aa";
        assert_eq!(with_name(first, "Outro"), first);
        assert_eq!(with_name(BARE, "  "), BARE);
        assert_eq!(with_name("", "Titulo"), "");
    }
}
