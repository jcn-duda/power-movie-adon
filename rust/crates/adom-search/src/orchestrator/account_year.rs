//! Ano do ARQUIVO contra o catálogo, no inventário da conta.

use super::{InventoryItem, SearchMeta};

/// Filme cujo MAIOR vídeo cita um ano que contradiz o catálogo (±2): o
/// título pode não ter ano e enganar o filtro (o remake de 1990 saía como
/// fonte do Night of the Living Dead de 1968). Pack multiobra fica: o maior
/// arquivo é só uma das obras. Série fica: o ano do arquivo é o da temporada.
pub(super) fn file_year_contradicts(item: &InventoryItem, meta: &SearchMeta, year: &str) -> bool {
    let Some(file) = item.main_file.as_deref() else {
        return false;
    };
    let catalog: i64 = year
        .as_bytes()
        .windows(4)
        .find(|w| w.iter().all(u8::is_ascii_digit))
        .and_then(|w| std::str::from_utf8(w).ok()?.parse().ok())
        .unwrap_or(0);
    meta.season.is_none()
        && !adom_core::is_multi_work_collection(&item.title)
        && adom_core::text_years_contradict(file, catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, file: Option<&str>) -> InventoryItem {
        InventoryItem {
            title: title.into(),
            main_file: file.map(str::to_string),
            ..InventoryItem::default()
        }
    }

    #[test]
    fn remake_de_1990_nao_e_o_filme_de_1968() {
        let meta = SearchMeta::default();
        let remake = item(
            "Night of the Living Dead Dublado mp4 1080p",
            Some("A Noite dos Mortos Vivos (1990) BDrip 1080p Dublado dat2014.mp4"),
        );
        assert!(file_year_contradicts(&remake, &meta, "1968"));
        assert!(!file_year_contradicts(&remake, &meta, "1990"));
        // Sem ano no arquivo ou sem lista de arquivos: sem prova, fica.
        let sem_ano = item(
            "Night of the Living Dead",
            Some("night.of.the.living.dead.mkv"),
        );
        assert!(!file_year_contradicts(&sem_ano, &meta, "1968"));
        assert!(!file_year_contradicts(
            &item("Night of the Living Dead", None),
            &meta,
            "1968"
        ));
    }

    #[test]
    fn pack_multiobra_fica() {
        let pack = item(
            "Evil Dead Collection 1981-2026 REMASTERED 1080p BluRay",
            Some("Evil Dead 2013 EXTENDED REMASTERED 1080p BluRay.mkv"),
        );
        assert!(!file_year_contradicts(
            &pack,
            &SearchMeta::default(),
            "1981"
        ));
    }
}
