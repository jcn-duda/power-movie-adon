import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createAftvResolver, extractDestination } from '../src/utils/aftv-resolve.js';

// Códigos da AFTVnews resolvidos pelo FlareSolverr (o site barra apps com o
// desafio do Cloudflare desde 2026-11-12).

const DEST = 'https://powermovie.net/abc/manifest.json';

function solver(solution: { url: string; response: string } | null, status = 'ok') {
  const calls: any[] = [];
  const fetchImpl = async (_url: string, init?: RequestInit) => {
    calls.push(JSON.parse(String(init?.body)));
    return new Response(JSON.stringify({ status, message: 'x', solution }), { status: 200 });
  };
  return { calls, fetchImpl };
}

test('destino vem do solution.url depois do redirect em JS', async () => {
  const s = solver({ url: DEST, response: '<pre>{"id":"x"}</pre>' });
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl: s.fetchImpl });
  assert.deepEqual(await resolver.resolve('7330456'), { url: DEST });
  assert.equal(s.calls[0].url, 'https://go.aftvnews.com/7330456');
  assert.equal(s.calls[0].cmd, 'request.get');
});

test('código inexistente vira url null', () => {
  assert.deepEqual(
    extractDestination('https://go.aftvnews.com/99999', 'The Short URL you entered does not exist.'),
    { url: null },
  );
});

test('sem redirect seguido, lê o window.location da página', () => {
  assert.deepEqual(
    extractDestination('https://go.aftvnews.com/1', 'window.location = "https://a.example/manifest.json?x=1&amp;y=2";'),
    { url: 'https://a.example/manifest.json?x=1&y=2' },
  );
  assert.equal(extractDestination('https://go.aftvnews.com/1', '<html>nada</html>'), 'unknown');
});

test('resultado fica em cache: o solver roda uma vez por código', async () => {
  const s = solver({ url: DEST, response: '' });
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl: s.fetchImpl });
  await resolver.resolve('7330456');
  await resolver.resolve('7330456');
  assert.equal(s.calls.length, 1);
});

test('falha do solver vira erro', async () => {
  const s = solver(null, 'error');
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl: s.fetchImpl });
  await assert.rejects(resolver.resolve('7330456'), /FlareSolverr/);
});

test('um código por vez no solver (padrão): o segundo diferente recebe busy', async () => {
  let release!: () => void;
  const gate = new Promise<void>((r) => { release = r; });
  const fetchImpl = async () => {
    await gate;
    return new Response(JSON.stringify({ status: 'ok', solution: { url: DEST, response: '' } }));
  };
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl });
  const a = resolver.resolve('111');
  assert.equal(resolver.isCached('111'), true, 'em voo não custa outra vaga');
  await assert.rejects(resolver.resolve('222'), /busy/);
  release();
  await a;
  assert.equal(resolver.isCached('111'), true);
  assert.equal(resolver.isCached('222'), false);
});

test('com maxConcurrent 2, o terceiro código diferente recebe busy', async () => {
  let release!: () => void;
  const gate = new Promise<void>((r) => { release = r; });
  const fetchImpl = async () => {
    await gate;
    return new Response(JSON.stringify({ status: 'ok', solution: { url: DEST, response: '' } }));
  };
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl, maxConcurrent: 2 });
  const a = resolver.resolve('111');
  const b = resolver.resolve('222');
  await assert.rejects(resolver.resolve('333'), /busy/);
  release();
  await Promise.all([a, b]);
});

test('AFTVnews sobrecarregada: tenta de novo e resolve quando ela volta', async () => {
  const bodies = ['The server is under heavy load. Please try again in a few minutes.', 'window.location = "' + DEST + '";'];
  let calls = 0;
  const fetchImpl = async () => {
    const response = bodies[Math.min(calls++, bodies.length - 1)];
    return new Response(JSON.stringify({ status: 'ok', solution: { url: 'https://go.aftvnews.com/8508163', response } }));
  };
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl, retryDelayMs: 0 });
  assert.deepEqual(await resolver.resolve('8508163'), { url: DEST });
  assert.equal(calls, 2);
});

test('AFTVnews sobrecarregada nas 3 tentativas: erro overloaded e nada em cache', async () => {
  let calls = 0;
  const fetchImpl = async () => {
    calls++;
    return new Response(JSON.stringify({ status: 'ok', solution: { url: 'https://go.aftvnews.com/1', response: 'under heavy load' } }));
  };
  const resolver = createAftvResolver({ flaresolverrUrl: 'http://fs/v1', fetchImpl, retryDelayMs: 0 });
  await assert.rejects(resolver.resolve('8508163'), /overloaded/);
  assert.equal(calls, 3);
  await assert.rejects(resolver.resolve('8508163'), /overloaded/);
  assert.equal(calls, 6, 'falha não fica em cache');
});

test('a própria AFTVnews (http, https ou www) nunca é destino', () => {
  // Medido em 2026-11-15: o solver parou em http://go.aftvnews.com/<código>
  // antes do redirect e o app recebia a AFTVnews como link de instalação.
  for (const url of ['http://go.aftvnews.com/9137024', 'https://www.aftvnews.com/x', 'https://aftvnews.com/']) {
    assert.equal(extractDestination(url, '<html>carregando</html>'), 'unknown', url);
  }
  assert.deepEqual(extractDestination(DEST, ''), { url: DEST });
});
