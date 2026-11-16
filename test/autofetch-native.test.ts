// Ponte Node → worker NATIVO do Chupim (`/autofetch*` da admin do `adom-server`).
//
// O worker, a fila e a credencial vivem no Rust (`cargo test -p adom-server`);
// o que o Node precisa garantir é o CONTRATO da ponte: token SÓ no header, URL
// sem segredo, status normalizado, e `null`/`{ok:false}` — nunca exceção — no
// caminho do painel. Nada de rede real: o `fetch` é dublado em todos os testes.
import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';

process.env.CACHE_PERSIST = 'false';

import config from '../src/config.js';
import {
  autofetchEffectApplied,
  drainAutofetch,
  drainSuppressedAutofetch,
  fetchAutofetchStatus,
  invalidateAutofetchStatus,
  pauseAutofetch,
  resetAutofetchBridgeForTests,
  setAutofetchConfig,
} from '../src/utils/autofetch-native.js';
import { stubFetch, type FetchResponse } from './helpers/stub.js';

const BASE = 'http://127.0.0.1:8778';
const TOKEN = 'tok-admin-chupim';
const saved = {
  url: config.streamStore.url,
  token: config.streamStore.token,
  timeoutMs: config.streamStore.timeoutMs,
  backoffMs: config.streamStore.backoffMs,
};

beforeEach(() => {
  resetAutofetchBridgeForTests();
  config.streamStore.url = BASE;
  config.streamStore.token = TOKEN;
  config.streamStore.timeoutMs = 200;
  // Backoff alto para o teste não depender de relógio: bloqueada, a ponte fica
  // bloqueada até o `restore` do finally.
  config.streamStore.backoffMs = 60_000;
});

afterEach(() => {
  resetAutofetchBridgeForTests();
  config.streamStore.url = saved.url;
  config.streamStore.token = saved.token;
  config.streamStore.timeoutMs = saved.timeoutMs;
  config.streamStore.backoffMs = saved.backoffMs;
});

function json(body: unknown): FetchResponse {
  return { ok: true, status: 200, json: async () => body };
}

function timeoutError(): Error {
  return Object.assign(new Error('The operation was aborted due to timeout'), { name: 'TimeoutError' });
}

let stubCalls: Array<{ url: string; options?: any }> = [];

function withStub(handler: Parameters<typeof stubFetch>[0]) {
  const stub = stubFetch(handler);
  stubCalls = stub.calls;
  return stub;
}

function headersOf(index: number): Record<string, string> {
  return (stubCalls[index].options?.headers || {}) as Record<string, string>;
}

// (a) token só no header — URL limpa, sem segredo em lugar nenhum. O gateway
// velho (sem os campos novos do worker) ganha defaults, nunca undefined.
test('status: lê o contrato da admin e o token viaja SÓ no header', async () => {
  const stub = withStub(() => json({
    ok: true,
    status: {
      enabled: false, phase: 'prepared', paused: true,
      queueDepth: 3, lots: 2, inflight: false, intervalMs: 60_000,
    },
  }));
  try {
    const snap = await fetchAutofetchStatus();
    assert.deepEqual(snap, {
      enabled: false, phase: 'prepared', paused: true,
      queueDepth: 3, lots: 2, inflight: false, intervalMs: 60_000,
      processed: 0, skips: {}, lastError: null, vaultAccounts: 0,
    });
    assert.equal(stub.calls.length, 1);
    assert.equal(stub.calls[0].url, `${BASE}/autofetch`, 'GET no contrato da admin');
    assert.equal(stub.calls[0].url.includes(TOKEN), false, 'a URL não carrega o token');
    assert.equal(stub.calls[0].url.includes('token='), false, 'nada de token na querystring');
    assert.equal(stub.calls[0].options?.method, 'GET');
    assert.equal(headersOf(0)['x-indexer-test-token'], TOKEN, 'token no header da admin');
  } finally {
    stub.restore();
  }
});

test('status: admin antiga (sem o objeto status) não vira estado inventado', async () => {
  const stub = withStub(() => json({ ok: true }));
  try {
    assert.equal(await fetchAutofetchStatus(), null, 'sem `status` a ponte não responde o bloco');
  } finally {
    stub.restore();
  }
});

// (b) 503 do worker e prazo vencido: fallback silencioso, sem lançar.
test('503 do worker: comandos viram {ok:false} e o status vira null, sem exceção', async () => {
  const stub = withStub(() => ({
    ok: false,
    status: 503,
    json: async () => ({ ok: false, error: 'autofetch_worker_unavailable' }),
  }));
  try {
    assert.equal(await fetchAutofetchStatus(), null);
    const pause = await pauseAutofetch(true);
    assert.deepEqual(pause, { ok: false, error: 'autofetch_worker_unavailable' });
    assert.equal(autofetchEffectApplied(pause), false, 'recusa não é efeito aplicado');
    assert.equal(autofetchEffectApplied(await drainAutofetch()), false);
  } finally {
    stub.restore();
  }
});

test('prazo vencido/rede fora: TODAS as chamadas devolvem null e nada lança', async () => {
  const stub = withStub(() => { throw timeoutError(); });
  try {
    assert.equal(await fetchAutofetchStatus(), null);
    assert.equal(await pauseAutofetch(true), null);
    assert.equal(await setAutofetchConfig({ paused: false }), null);
    assert.equal(await drainAutofetch(), null);
    assert.equal(await drainSuppressedAutofetch(), null);
    // Sem exceção em nenhuma das cinco; e o backoff (2 falhas) poupa a rede do
    // resto em vez de insistir a cada clique do painel.
    assert.equal(stub.calls.length, 2, 'duas tentativas e o resto fica em backoff');
  } finally {
    stub.restore();
  }
});

test('backoff: duas falhas seguidas sossegam a ponte e o pedido seguinte nem sai', async () => {
  const failing = withStub(() => ({ ok: false, status: 500, json: async () => ({}) }));
  try {
    await fetchAutofetchStatus();
    await fetchAutofetchStatus();
    assert.equal(failing.calls.length, 2, 'as duas primeiras tentativas saíram');
    assert.equal(await drainAutofetch(), null, 'em backoff a ponte devolve null sem fetch');
    assert.equal(failing.calls.length, 2, 'e não faz fetch durante o bloqueio');
  } finally {
    failing.restore();
  }
  resetAutofetchBridgeForTests();
  const ok = withStub(() => json({ ok: true, status: { enabled: false, phase: 'prepared' } }));
  try {
    assert.ok(await fetchAutofetchStatus(), 'depois do reset o status volta a sair');
    assert.equal(ok.calls.length, 1);
  } finally {
    ok.restore();
  }
});

// (c) proxy das ações: caminho, corpo e header de cada rota da admin.
test('comandos: cada ação bate na rota certa da admin, com o corpo do contrato', async () => {
  const stub = withStub((rawUrl) => {
    if (rawUrl.endsWith('/autofetch/pause')) {
      return json({ ok: true, status: { enabled: false, phase: 'prepared', paused: true, intervalMs: 1000 } });
    }
    if (rawUrl.endsWith('/autofetch/config')) {
      return json({ ok: true, status: { enabled: false, phase: 'prepared', paused: false, intervalMs: 5000 } });
    }
    if (rawUrl.endsWith('/autofetch/drain')) {
      return json({ ok: true, enabled: false, phase: 'prepared', operation: 'drain', effect: 'none', drained: 0 });
    }
    if (rawUrl.endsWith('/autofetch/suppressed-drain')) {
      return json({ ok: true, enabled: false, phase: 'prepared', operation: 'suppressed-drain', effect: 'none', drained: 0 });
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  try {
    const pause = await pauseAutofetch(true);
    assert.equal(pause?.ok, true);
    assert.equal(pause?.status?.paused, true, 'estado efetivo vem do gateway');
    assert.deepEqual(JSON.parse(String(stub.calls[0].options?.body)), { paused: true });

    const configOut = await setAutofetchConfig({ paused: false, intervalMs: 5000 });
    assert.equal(configOut?.status?.intervalMs, 5000);
    assert.deepEqual(JSON.parse(String(stub.calls[1].options?.body)), { paused: false, intervalMs: 5000 });

    const drain = await drainAutofetch();
    assert.deepEqual(
      { ok: drain?.ok, effect: drain?.effect, operation: drain?.operation, drained: drain?.drained },
      { ok: true, effect: 'none', operation: 'drain', drained: 0 },
    );
    assert.deepEqual(JSON.parse(String(stub.calls[2].options?.body)), {}, 'o dreno não manda credencial nem parâmetro');

    const suppressed = await drainSuppressedAutofetch();
    assert.equal(suppressed?.operation, 'suppressed-drain');

    assert.deepEqual(
      stub.calls.map((call) => call.url),
      [`${BASE}/autofetch/pause`, `${BASE}/autofetch/config`, `${BASE}/autofetch/drain`, `${BASE}/autofetch/suppressed-drain`],
    );
    for (const [index, call] of stub.calls.entries()) {
      assert.equal(call.options?.method, 'POST');
      assert.equal(headersOf(index)['x-indexer-test-token'], TOKEN, `token no header (${call.url})`);
      assert.equal(call.url.includes(TOKEN), false, `URL sem token (${call.url})`);
    }
  } finally {
    stub.restore();
  }
});

test('effect: só `effect` diferente de "none" libera o Node de fazer o trabalho', () => {
  assert.equal(autofetchEffectApplied({ ok: true, effect: 'none' }), false);
  assert.equal(autofetchEffectApplied({ ok: true }), true, 'worker vivo devolve drained sem effect');
  assert.equal(autofetchEffectApplied({ ok: false, effect: 'applied' }), false, 'recusa nunca conta');
  assert.equal(autofetchEffectApplied(null), false);
});

// (d) diagnóstico do worker: o que o Rust publica não pode ser descartado no
// caminho do painel — métrica que chega e some é diagnóstico jogado fora.
test('status: processed/skips/lastError/vaultAccounts chegam com coerção defensiva', async () => {
  const stub = withStub(() => json({
    ok: true,
    status: {
      enabled: true, phase: 'active', paused: false,
      queueDepth: 1, lots: 1, inflight: true, intervalMs: 60_000,
      processed: 12, skips: { 'no-hash': 3, budget: 1 },
      lastError: 'store indisponível', vaultAccounts: 2,
    },
  }));
  try {
    const snap = await fetchAutofetchStatus();
    assert.equal(snap?.processed, 12);
    assert.deepEqual(snap?.skips, { 'no-hash': 3, budget: 1 });
    assert.equal(snap?.lastError, 'store indisponível');
    assert.equal(snap?.vaultAccounts, 2);
  } finally {
    stub.restore();
  }
  resetAutofetchBridgeForTests();
  const estranho = withStub(() => json({
    ok: true,
    status: { enabled: true, processed: '7', skips: ['no-hash'], lastError: 42, vaultAccounts: null },
  }));
  try {
    const snap = await fetchAutofetchStatus();
    assert.equal(snap?.processed, 7, 'string numérica é coagida');
    assert.deepEqual(snap?.skips, {}, 'array não é mapa motivo→contagem');
    assert.equal(snap?.lastError, null, 'não-string não vira erro inventado');
    assert.equal(snap?.vaultAccounts, 0);
  } finally {
    estranho.restore();
  }
});

// (e) memo curto + dedupe: o poll de ~10s não paga uma GET por render, e `null`
// (admin fora) NUNCA vira memo de 30s que esconderia a recuperação.
test('memo: dentro do TTL serve a foto sem novo GET; null não esconde a recuperação', async () => {
  const ok = withStub(() => json({ ok: true, status: { enabled: false, phase: 'prepared' } }));
  try {
    const first = await fetchAutofetchStatus();
    const second = await fetchAutofetchStatus();
    assert.equal(first, second, 'a mesma foto dentro da janela');
    assert.equal(ok.calls.length, 1, 'o segundo poll não foi à rede');
  } finally {
    ok.restore();
  }
  invalidateAutofetchStatus();
  const fora = withStub(() => { throw timeoutError(); });
  try {
    assert.equal(await fetchAutofetchStatus(), null, 'admin caiu');
  } finally {
    fora.restore();
  }
  const volta = withStub(() => json({ ok: true, status: { enabled: true, phase: 'active' } }));
  try {
    assert.equal((await fetchAutofetchStatus())?.phase, 'active', 'null não foi memoizado: a recuperação aparece');
    assert.equal(volta.calls.length, 1);
  } finally {
    volta.restore();
  }
});

test('dedupe: polls simultâneos pagam uma GET só', async () => {
  const stub = withStub(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
    return json({ ok: true, status: { enabled: false, phase: 'prepared' } });
  });
  try {
    const [a, b] = await Promise.all([fetchAutofetchStatus(), fetchAutofetchStatus()]);
    assert.equal(a, b, 'mesma foto para os dois polls');
    assert.equal(stub.calls.length, 1, 'o pedido em voo é compartilhado');
  } finally {
    stub.restore();
  }
});

test('ação do operador invalida o memo: o poll seguinte reflete a pausa', async () => {
  let paused = false;
  const stub = withStub((rawUrl) => {
    if (rawUrl.endsWith('/autofetch/pause')) {
      paused = true;
      return json({ ok: true, status: { enabled: true, phase: 'active', paused: true } });
    }
    return json({ ok: true, status: { enabled: true, phase: 'active', paused } });
  });
  try {
    assert.equal((await fetchAutofetchStatus())?.paused, false);
    assert.equal((await fetchAutofetchStatus())?.paused, false, 'memo antes da ação');
    assert.equal(stub.calls.length, 1);
    assert.equal((await pauseAutofetch(true))?.ok, true);
    assert.equal((await fetchAutofetchStatus())?.paused, true, 'o poll pós-ação sai à rede e vê o estado novo');
    assert.equal(stub.calls.length, 3, 'GET, POST e novo GET');
  } finally {
    stub.restore();
  }
});

test('recusa da admin não invalida o memo (nada foi aplicado)', async () => {
  const stub = withStub((rawUrl) => {
    if (rawUrl.endsWith('/autofetch/pause')) {
      return { ok: false, status: 503, json: async () => ({ ok: false, error: 'autofetch_worker_unavailable' }) };
    }
    return json({ ok: true, status: { enabled: false, phase: 'prepared' } });
  });
  try {
    await fetchAutofetchStatus();
    assert.equal((await pauseAutofetch(true))?.ok, false);
    await fetchAutofetchStatus();
    assert.equal(stub.calls.length, 2, 'GET + POST; o poll pós-recusa usou o memo');
  } finally {
    stub.restore();
  }
});
