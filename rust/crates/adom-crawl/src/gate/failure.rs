//! Falha do pagamento pelo resolver. Separado do `gate.rs` pela catraca de
//! 400 linhas; continua sendo `impl GateExecutor`.

use super::{with_store, GateExecutor, RowOutcome};
use crate::store::ops::GateRow;

/// Tentativas de pagamento a partir de que um portão com veredito "expirado"
/// vira TERMINAL. Medido em 2026-11-15: o TTL do portão é ~26–31 dias e os
/// portões antigos (host1, 26–31 dias de idade) respondem "Referência
/// expirada" — 3/3 sondados mortos. Insistir neles queima FlareSolverr (~30 s
/// por chamada) e o teto por hora na frente dos portões novos que pagariam.
const GATE_EXPIRED_MAX_TRIES: u32 = 3;

impl GateExecutor {
    /// O resolver recusou o pagamento. O custo é real (a tentativa aconteceu):
    /// cobrado no teto. "pendente" da cadeia em curso não é falha — repete na
    /// rodada seguinte sem recuo. As demais falhas recuam (`defer_delivery`);
    /// sem recuo, a cabeça da fila comia o teto por hora para sempre.
    ///
    /// Exceção: veredito "expirado" PERSISTENTE (≥ [`GATE_EXPIRED_MAX_TRIES`]
    /// tentativas) é terminal — o link do portão morreu e não volta. A linha
    /// vira `error` (revive por `lastmod` novo ou "Reprocessar erros") e o
    /// portão sai da fila.
    pub(super) async fn payment_failed(&self, g: &GateRow, erro: &str) -> RowOutcome {
        eprintln!(
            "[gate] {} {}: pagamento não saiu: {erro}",
            g.row.site, g.row.url_key
        );
        self.state.lock().await.spent_this_hour += 1;
        if erro.starts_with("pendente") {
            return RowOutcome::Pending;
        }
        let tentativas = g.row.tries.saturating_add(1);
        if erro.contains("expirado") && tentativas >= GATE_EXPIRED_MAX_TRIES {
            let (site, url_key, err) =
                (g.row.site.clone(), g.row.url_key.clone(), erro.to_string());
            if let Err(e) = with_store(self.store.clone(), move |db| {
                db.expire_gate(&site, &url_key, &err)
            })
            .await
            {
                eprintln!(
                    "[gate] {} {}: veredito terminal não gravado: {e}",
                    g.row.site, g.row.url_key
                );
                return RowOutcome::Pending;
            }
            eprintln!(
                "[gate] {} {}: portão expirado após {tentativas} tentativa(s) — terminal",
                g.row.site, g.row.url_key
            );
            return RowOutcome::Expired;
        }
        self.defer_delivery(g, "pagamento", erro).await
    }
}
