//! Provas do `NativeKv` na tabela `cache`: TTL 0 é permanente, expirado some
//! na leitura e a varredura por prefixo apaga o que venceu no caminho.

use serde_json::json;

use super::NativeKv;

#[test]
fn faxina_do_boot_apaga_velho_legado_e_vencido_sem_dono() {
    let kv = NativeKv::open_memory().unwrap();
    let now = 10_000_000;
    let live = now + 1_000_000;
    for (key, expires) in [
        // Versão velha some mesmo sem ter vencido; versão atual e futura ficam.
        ("rdc:v1:velho", live),
        ("rdc:v2:atual", live),
        ("rdc:v9:do-futuro", live),
        ("idx:v1:dono-proprio", live),
        // Legado do núcleo e aposentado na F2.
        ("raw1:velho", live),
        ("rdt:v1:orfao", live),
        ("dinv:v1:orfao", live),
        // Vencido da lista fechada sai; vivo, permanente e fora da lista ficam.
        ("tmdb:v1:vencido", now - 1),
        ("tmdb:v1:vivo", live),
        ("cfg:v1:permanente", 0),
        ("adrm:v1:vencido", now - 1),
        ("adsub:v1:vencido", now - 1),
        ("adsub:v1:vivo", live),
        ("adhold:v1:vencido", now - 1),
        ("autofetch:v3:m:vencido", now - 1),
        ("autofetch:v3:q:permanente", 0),
        ("autofetch-worker:v1:budget:vencido", now - 1),
        ("autofetch-worker:v1:config", 0),
        ("harvest:v1:seen:vencido", now - 1),
        ("harvest:v1:q", 0),
        ("adprot:v1:vencido-fora-da-lista", now - 1),
        ("metam:v1:counts", 0),
    ] {
        kv.lock()
            .execute(
                "INSERT INTO cache (key, value, expires_at) VALUES (?1, '1', ?2)",
                rusqlite::params![key, expires],
            )
            .unwrap();
    }

    let done = kv.boot_housekeeping(now).unwrap();

    assert_eq!(
        done,
        super::BootHousekeeping {
            stale: 1,
            legacy: 3,
            expired: 7
        }
    );
    let conn = kv.lock();
    let mut stmt = conn.prepare("SELECT key FROM cache ORDER BY key").unwrap();
    let left: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        left,
        [
            "adprot:v1:vencido-fora-da-lista",
            "adsub:v1:vivo",
            "autofetch-worker:v1:config",
            "autofetch:v3:q:permanente",
            "cfg:v1:permanente",
            "harvest:v1:q",
            "idx:v1:dono-proprio",
            "metam:v1:counts",
            "rdc:v2:atual",
            "rdc:v9:do-futuro",
            "tmdb:v1:vivo",
        ]
    );
}

#[test]
fn ttl_zero_persiste_e_expirado_some_na_leitura() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("autofetch:q", &json!([1]), 0, 1_000).unwrap();
    assert!(kv.get_json("autofetch:q", 10_000_000).unwrap().is_some());
    kv.put_json("autofetch:x", &json!(1), 1, 1_000).unwrap();
    assert!(kv.get_json("autofetch:x", 2_000).unwrap().is_none());
}

#[test]
fn forget_remove_so_a_chave_pedida_e_e_idempotente() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("autofetch:v1:q", &json!([]), 0, 1_000).unwrap();
    kv.put_json("autofetch:v1:hold:x", &json!({}), 0, 1_000)
        .unwrap();

    assert!(kv.forget("autofetch:v1:q").unwrap());
    assert!(!kv.forget("autofetch:v1:q").unwrap(), "idempotente");
    assert!(
        kv.get_json("autofetch:v1:hold:x", 2_000).unwrap().is_some(),
        "vizinha de prefixo não é tocada"
    );
}

#[test]
fn keys_matching_por_prefixo_apaga_expirado() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("autofetch:obra:tt1", &json!({"a": 1}), 0, 1_000)
        .unwrap();
    // expira em 2_000: a varredura em 2_000 já a considera vencida.
    kv.put_json("autofetch:suppressed:tt2", &json!(2), 1, 1_000)
        .unwrap();
    kv.put_json("rdq:hash", &json!(3), 0, 1_000).unwrap();

    let vivas = kv.keys_matching("autofetch:", 2_000).unwrap();
    assert_eq!(
        vivas,
        vec!["autofetch:obra:tt1".to_string()],
        "só a viva volta, e prefixo alheio fica fora"
    );

    // A expirada foi APAGADA na varredura: com relógio anterior à expiração
    // ela ainda seria "viva" se a linha existisse.
    let restantes = kv.keys_matching("autofetch:", 1_000).unwrap();
    assert_eq!(
        restantes,
        vec!["autofetch:obra:tt1".to_string()],
        "a linha expirada não volta nem com o relógio recuado"
    );

    assert!(
        kv.get_json("rdq:hash", 9_999_999).unwrap().is_some(),
        "prefixo alheio não é tocado pela varredura"
    );
}

#[test]
fn remaining_seconds_segue_a_regua_do_peek_remaining() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("adprot:v1:alldebrid:a:h1", &json!({"a": 1}), 3_600, 1_000)
        .unwrap();
    kv.put_json("adprot:v1:alldebrid:a:h2", &json!({"a": 1}), 0, 1_000)
        .unwrap();

    // 3_599_500 ms restantes = 3599,5 s: o Node arredonda para 3600.
    assert_eq!(
        kv.remaining_seconds("adprot:v1:alldebrid:a:h1", 1_500)
            .unwrap(),
        Some(3_600)
    );
    assert_eq!(
        kv.remaining_seconds("adprot:v1:alldebrid:a:h2", 1_500)
            .unwrap(),
        None,
        "permanente não tem o que renovar"
    );
    assert_eq!(
        kv.remaining_seconds("adprot:v1:alldebrid:a:h1", 4_000_000)
            .unwrap(),
        None,
        "vencida devolve None sem apagar"
    );
    assert!(
        kv.get_json("adprot:v1:alldebrid:a:h1", 1_600)
            .unwrap()
            .is_some(),
        "a sonda de TTL não apaga a linha"
    );
}

#[test]
fn forget_prefix_apaga_so_o_namespace_e_conta_linhas() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("adsub:v1:a:h1", &json!({"at": 1}), 0, 1_000)
        .unwrap();
    kv.put_json("adsub:v1:b:h2", &json!({"at": 2}), 0, 1_000)
        .unwrap();
    kv.put_json("adsub:v1x:a:h3", &json!({"at": 3}), 0, 1_000)
        .unwrap();

    assert_eq!(kv.forget_prefix("adsub:v1:").unwrap(), 2);
    assert_eq!(kv.forget_prefix("adsub:v1:").unwrap(), 0, "idempotente");
    assert!(
        kv.get_json("adsub:v1x:a:h3", 2_000).unwrap().is_some(),
        "prefixo mais longo não casa"
    );
}

#[test]
fn prune_expired_apaga_so_vencido_do_prefixo() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("fsz:v1:a", &json!([1]), 1, 1_000).unwrap(); // vence em 2_000
    kv.put_json("fsz:v1:b", &json!([2]), 0, 1_000).unwrap(); // permanente
    kv.put_json("fsz:v1:c", &json!([3]), 60, 1_000).unwrap(); // vivo até 61_000
    kv.put_json("tsz:v1:x", &json!(9), 1, 1_000).unwrap(); // outro prefixo

    assert_eq!(kv.prune_expired("fsz:v1:", 5_000).unwrap(), 1);
    assert_eq!(
        kv.prune_expired("fsz:v1:", 5_000).unwrap(),
        0,
        "idempotente"
    );
    assert!(kv.get_json("fsz:v1:b", 5_000).unwrap().is_some());
    assert!(kv.get_json("fsz:v1:c", 5_000).unwrap().is_some());
    // Outro prefixo não é tocado: ainda vivo em t=1_500 (expira em 2_000).
    assert!(kv.get_json("tsz:v1:x", 1_500).unwrap().is_some());
}

#[test]
fn get_many_json_pula_ausente_vencido_e_json_invalido() {
    // Linha com JSON inválido só entra por SQL cru: a API nunca grava isso.
    let path = std::env::temp_dir().join(format!("adom-kv-many-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let kv = NativeKv::open(&path).unwrap();
    kv.put_json("fsz:v1:ok", &json!([1, 2]), 0, 1_000).unwrap();
    kv.put_json("fsz:v1:velho", &json!(1), 1, 1_000).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO cache (key, value, expires_at) VALUES ('fsz:v1:ruim', '{nao-json', 0)",
            [],
        )
        .unwrap();

    let keys: Vec<String> = ["fsz:v1:ok", "fsz:v1:velho", "fsz:v1:ruim", "fsz:v1:ausente"]
        .iter()
        .map(|key| key.to_string())
        .collect();
    let rows = kv.get_many_json(&keys, 5_000).unwrap();
    assert_eq!(rows, vec![("fsz:v1:ok".to_string(), json!([1, 2]))]);
    drop(kv);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn put_and_forget_rollback_ambas_as_operacoes_se_a_remocao_falhar() {
    let kv = NativeKv::open_memory().unwrap();
    kv.put_json("adhold:v1:account:hash", &json!({"at": 1}), 60, 1_000)
        .unwrap();
    kv.conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_hold_delete BEFORE DELETE ON cache
             WHEN OLD.key = 'adhold:v1:account:hash'
             BEGIN SELECT RAISE(ABORT, 'injected delete failure'); END;",
        )
        .unwrap();

    assert!(kv
        .put_many_json_and_forget(
            &[super::KvWrite {
                key: "adsub:v1:account:hash",
                value: &json!({"at": 2}),
                ttl_seconds: 600,
                now_ms: 2_000,
                only_if_absent: false,
            }],
            &["adhold:v1:account:hash"],
        )
        .is_err());
    assert!(kv
        .get_json("adsub:v1:account:hash", 2_001)
        .unwrap()
        .is_none());
    assert!(kv
        .get_json("adhold:v1:account:hash", 2_001)
        .unwrap()
        .is_some());
}

/// Chaves do banco que casam o prefixo, em ordem. Referência: `starts_with`,
/// o equivalente em Rust do `substr(key, 1, length(prefix))` do SQLite para
/// texto UTF-8 válido (prefixo vazio casa tudo).
fn chaves_com(kv: &NativeKv, prefix: &str) -> Vec<String> {
    let conn = kv.lock();
    let mut stmt = conn.prepare("SELECT key FROM cache ORDER BY key").unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .filter(|k| k.starts_with(prefix))
        .collect()
}

/// Paridade com a semântica ANTIGA (`substr`): prefixo arbitrário (chars
/// altos, teto U+10FFFF, vazio), ordem por chave, permanentes ficam na poda e
/// vencidas saem na listagem. A referência é o `starts_with` (o `substr` do
/// SQLite casa por caractere em texto UTF-8 válido).
#[test]
#[rustfmt::skip]
fn varreduras_tem_paridade_com_o_substr_antigo() {
    use super::prefix_range;
    // Bordas do sucessor: carry no teto, pulo dos surrogates, `None` sem sucessor.
    for (prefix, hi) in [
        ("é", Some("ê")),
        ("x\u{D7FF}", Some("x\u{E000}")),
        ("x\u{10FFFF}", Some("y")),
        ("", None),
        ("\u{10FFFF}", None),
        ("autofetch:v3:o:", Some("autofetch:v3:o;")),
    ] {
        assert_eq!(prefix_range(prefix), (prefix.to_string(), hi.map(str::to_string)));
    }
    const NOW: i64 = 10_000;
    let rows: &[(&str, i64)] = &[
        ("", 0),
        ("autofetch:v3:o:", 0),
        ("autofetch:v3:o:tt1", NOW + 1_000),
        ("autofetch:v3:o:tt2", NOW - 1),
        ("autofetch:v3:o:é", NOW + 1_000),
        ("autofetch:v3:o;", NOW + 1_000),
        ("autofetch:v3:p:x", NOW + 1_000),
        ("é", NOW + 1_000),
        ("éx", NOW + 1_000),
        ("ê", NOW + 1_000),
        ("x\u{10FFFF}", NOW + 1_000),
        ("x\u{10FFFF}\u{10FFFF}", NOW + 1_000),
        ("y", NOW + 1_000),
        ("\u{10FFFF}", NOW - 1),
        ("\u{10FFFF}\u{10FFFF}", NOW + 1_000),
        ("x\u{D7FF}", NOW + 1_000),
        ("x\u{E000}", NOW + 1_000),
    ];
    let seed = || {
        let kv = NativeKv::open_memory().unwrap();
        let conn = kv.lock();
        for (key, expires) in rows {
            conn.execute("INSERT INTO cache (key, value, expires_at) VALUES (?1, '1', ?2)",
                rusqlite::params![key, expires]).unwrap();
        }
        drop(conn);
        kv
    };
    for prefix in ["autofetch:v3:o:", "", "é", "x\u{10FFFF}", "\u{10FFFF}", "x\u{D7FF}", "zz:"] {
        let casam: Vec<_> = rows.iter().copied().filter(|(k, _)| k.starts_with(prefix)).collect();
        let mut vivas: Vec<_> = casam.iter().filter(|(_, e)| *e == 0 || *e > NOW).collect();
        vivas.sort();
        let vivas: Vec<String> = vivas.iter().map(|(k, _)| k.to_string()).collect();
        let vencidas = casam.iter().filter(|(_, e)| *e > 0 && *e <= NOW).count() as u32;
        let kv = seed();
        assert_eq!(kv.keys_matching(prefix, NOW).unwrap(), vivas, "lista {prefix:?}");
        assert_eq!(chaves_com(&kv, prefix), vivas, "poda na lista {prefix:?}");
        let kv = seed();
        assert_eq!(kv.prune_expired(prefix, NOW).unwrap(), vencidas, "poda {prefix:?}");
        assert_eq!(chaves_com(&kv, prefix), vivas, "permanentes ficam {prefix:?}");
        let kv = seed();
        assert_eq!(kv.forget_prefix(prefix).unwrap(), casam.len() as u32, "reset {prefix:?}");
        assert!(chaves_com(&kv, prefix).is_empty(), "nada do prefixo sobra");
        assert_eq!(chaves_com(&kv, "").len(), rows.len() - casam.len(), "só o prefixo sai");
    }
}

/// Prova de plano: a faixa BUSCA pela PK; o `substr` antigo varria a tabela
/// inteira (mesmo controle do teste de plano do `release_index`).
#[test]
#[rustfmt::skip]
fn plano_das_varreduras_do_kv_busca_na_primaria() {
    let kv = NativeKv::open_memory().unwrap();
    let conn = kv.lock();
    let plano = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> String {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        stmt.query_map(args, |row| row.get::<_, String>(3)).unwrap()
            .map(Result::unwrap).collect::<Vec<_>>().join(" | ")
    };
    let busca = |sql: &str, args: &[&dyn rusqlite::ToSql]| {
        let p = plano(sql, args);
        assert!(p.contains("SEARCH") && !p.contains("SCAN"), "faixa busca na PK: {p} ({sql})");
    };
    let (lo, hi, now) = ("autofetch:v3:o:", "autofetch:v3:o;", 10_000i64);
    busca("SELECT key FROM cache WHERE key >= ?1 AND key < ?2 ORDER BY key", &[&lo, &hi]);
    busca("SELECT key FROM cache WHERE key >= ?1 ORDER BY key", &[&lo]);
    busca("DELETE FROM cache WHERE key >= ?1 AND key < ?2", &[&lo, &hi]);
    busca("DELETE FROM cache WHERE expires_at > 0 AND expires_at <= ?1 AND key >= ?2 AND key < ?3",
        &[&now, &lo, &hi]);
    // Controle: a forma antiga nunca alcança a PRIMARY KEY.
    let velho = plano("SELECT key FROM cache WHERE substr(key, 1, length(?1)) = ?1", &[&lo]);
    assert!(velho.contains("SCAN"), "o substr sempre varreu: {velho}");
}
