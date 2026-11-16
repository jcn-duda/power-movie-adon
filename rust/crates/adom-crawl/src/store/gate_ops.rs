//! OperaÃ§Ãµes de portÃ£o do `crawl.db` (status `gate-pending` + tabela
//! `crawl_gate`). Separado do `ops.rs` pela catraca de 400 linhas; o
//! `impl CrawlStore` continua o mesmo tipo â€” sÃ³ a fonte Ã© outra.

use rusqlite::params;

use super::ops::{now_ms, row_from_sqlite, CrawlStore};
use super::types::{GatePendingRecord, GateRow, StoreError};

impl CrawlStore {
    /// Marca o post como PENDENTE DE PORTÃƒO (`gate-pending`) e grava o portÃ£o
    /// na `crawl_gate`.
    ///
    /// Diferente do `mark_result`: NÃƒO incrementa `tries` nem backoff â€” pendÃªncia
    /// de portÃ£o nÃ£o Ã© tentativa falha, Ã© custo deferido (FlareSolverr ~30s por
    /// post pago pelo executor de fundo). Envenenar a linha aqui seria punir o
    /// post pelo custo do site, nÃ£o por erro dele.
    pub fn mark_gate_pending(
        &self,
        site: &str,
        url_key: &str,
        rec: &GatePendingRecord,
    ) -> Result<(), StoreError> {
        let now = now_ms();
        self.conn().execute(
            "UPDATE crawl_url SET status='gate-pending', imdb=COALESCE(?3,imdb), \
             releases=0, error='', next_at=0, checked_at=?2 \
             WHERE site=?1 AND url_key=?4",
            params![site, now, rec.imdb, url_key],
        )?;
        self.conn().execute(
            "INSERT OR REPLACE INTO crawl_gate \
             (site, url_key, gate, title, year, imdb, kind, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![site, url_key, rec.gate, rec.title, rec.year, rec.imdb, rec.kind, now,],
        )?;
        Ok(())
    }

    /// Toma posts pendentes de portão (o MAIS NOVO primeiro, pelo `added_at`
    /// da marcação) para o executor de fundo. Fresh-first é o salvamento ótimo:
    /// o portão tem TTL medido de ~26–31 dias (2026-11-15), então o mais novo
    /// tem a maior chance de ainda estar vivo — o mais antigo primeiro drenava
    /// uma fila de expirados na frente dos portões novos que pagariam de fato.
    /// NÃO marca inflight: o executor é single-loop e a linha continua
    /// `gate-pending` durante o pagamento — se o processo morrer no meio, a
    /// pendência sobrevive.
    pub fn take_gate_batch(&self, limit: usize) -> Result<Vec<GateRow>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn().prepare(
            "SELECT u.site, u.url, u.url_key, u.lastmod, u.kind, u.status, u.imdb, u.tries, \
                    u.next_at, u.checked_at, u.releases, u.error, u.progress, u.added_at, \
                    g.gate, g.title, g.year, g.imdb, g.kind \
             FROM crawl_url u \
             JOIN crawl_gate g ON g.site = u.site AND g.url_key = u.url_key \
             WHERE u.status = 'gate-pending' AND u.next_at <= ?2 \
             ORDER BY g.added_at DESC, u.url_key \
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64, now_ms()], |row| {
            Ok(GateRow {
                row: row_from_sqlite(row)?,
                gate: row.get(14)?,
                title: row.get(15)?,
                year: row.get(16)?,
                imdb: row.get(17)?,
                kind: row.get(18)?,
            })
        })?;
        let collected: Result<Vec<_>, _> = rows.collect();
        Ok(collected?)
    }

    /// Recuo de uma ENTREGA que falhou (o Node recusou o resultado): a linha
    /// segue `gate-pending` — sem veneno, o portão continua pago — mas só volta
    /// ao lote em `next_at`. Sem isso, um post que o Node recusa sempre (obra
    /// sem nome no catálogo) era reenviado a cada rodada do executor.
    pub fn defer_gate_delivery(
        &self,
        site: &str,
        url_key: &str,
        next_at: i64,
        error: &str,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE crawl_url SET tries=tries+1, next_at=?3, error=?4 \
             WHERE site=?1 AND url_key=?2 AND status='gate-pending'",
            params![site, url_key, next_at, error],
        )?;
        Ok(())
    }

    /// Apaga o registro do portão de um post (pago com sucesso e gravado no
    /// acervo). A linha do post já saiu de `gate-pending` via `mark_result`.
    pub fn clear_gate(&self, site: &str, url_key: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "DELETE FROM crawl_gate WHERE site = ?1 AND url_key = ?2",
            params![site, url_key],
        )?;
        Ok(())
    }

    /// Veredito TERMINAL de portão expirado: a linha vira `error` com
    /// `next_at=0` (o "Reprocessar erros" do painel ainda a alcança; um
    /// `lastmod` novo também a revive pelo reset do `enqueue_discovered`) e o
    /// portão sai da fila — pagar de novo um link morto é queimar FlareSolverr.
    /// A tentativa que trouxe o veredito é debitada (`tries+1`), como no recuo.
    /// Só age em linha `gate-pending`: se ela já fechou, o portão não se toca.
    pub fn expire_gate(&self, site: &str, url_key: &str, error: &str) -> Result<(), StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE crawl_url SET status='error', next_at=0, tries=tries+1, error=?3 \
             WHERE site=?1 AND url_key=?2 AND status='gate-pending'",
            params![site, url_key, error],
        )?;
        if changed > 0 {
            tx.execute(
                "DELETE FROM crawl_gate WHERE site = ?1 AND url_key = ?2",
                params![site, url_key],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
