// Ponte do controle do raspador (`GET/POST /crawl/control` do sidecar Rust).
//
// O sidecar é o dono da pausa, do liga/desliga, do ritmo e do teto por hora
// (global e por site) e os grava no `crawl.db`. Antes os botões do painel só
// mudavam o estado do Node e o sidecar nunca os recebia (medido em 2026-11-15:
// o painel da VPS mostrava "desligado, só Vaca" com os nove sites raspando).
//
// A foto (`snapshot()`) é renovada junto com a do `crawl.db` no refresh do
// painel; sem resposta do sidecar ela é `null` e o painel cai na config local.
import config from '../config.js';

export interface CrawlControlSite {
  enabled?: boolean;
  paused?: boolean;
  delayMs?: number;
  maxPerHour?: number;
}

export interface CrawlControlState {
  enabled: boolean;
  paused: boolean;
  delayMs?: number;
  maxPerHour?: number;
  sites: Record<string, CrawlControlSite>;
}

export interface CrawlControlSnapshot {
  control: CrawlControlState;
  pagesThisHour: Record<string, number>;
  at: number;
}

/** Pedido ao sidecar. `null` apaga o override; `reset` volta o alvo ao padrão. */
export interface CrawlControlPatch {
  site?: string;
  paused?: boolean;
  enabled?: boolean | null;
  delayMs?: number | null;
  maxPerHour?: number | null;
  reset?: boolean;
}

export interface CrawlControlReply {
  ok: boolean;
  /** HTTP do sidecar; 0 = sidecar não acessível. */
  status: number;
  control?: CrawlControlState;
  error?: string;
}

let last: CrawlControlSnapshot | null = null;

function url(): string {
  return `http://127.0.0.1:${config.crawl.sidecarPort}/crawl/control`;
}

function headers(): Record<string, string> {
  return {
    'Content-Type': 'application/json',
    ...(config.jackett.testToken ? { 'X-Indexer-Test-Token': config.jackett.testToken } : {}),
  };
}

function remember(body: any): CrawlControlState | undefined {
  const control = body?.control;
  if (!control || typeof control !== 'object') return undefined;
  const state: CrawlControlState = {
    enabled: control.enabled !== false,
    paused: control.paused === true,
    sites: control.sites && typeof control.sites === 'object' ? control.sites : {},
    ...(typeof control.delayMs === 'number' ? { delayMs: control.delayMs } : {}),
    ...(typeof control.maxPerHour === 'number' ? { maxPerHour: control.maxPerHour } : {}),
  };
  const pages = body?.pagesThisHour && typeof body.pagesThisHour === 'object' ? body.pagesThisHour : {};
  last = { control: state, pagesThisHour: pages, at: Date.now() };
  return state;
}

/** Renova a foto. Falha mantém a anterior (o painel não pisca a cada timeout). */
export async function refreshControl(): Promise<void> {
  try {
    const resp = await fetch(url(), {
      headers: headers(),
      signal: AbortSignal.timeout(config.crawl.sidecarStatusTimeoutMs),
    });
    if (resp.ok) remember(await resp.json());
  } catch {
    // sidecar fora do ar: fica a última foto (ou null).
  }
}

/** Foto mais recente do controle; `null` sem nenhuma resposta do sidecar. */
export function controlSnapshot(): CrawlControlSnapshot | null {
  return last;
}

/** Envia o pedido ao sidecar e, no sucesso, já atualiza a foto. Nunca lança. */
export async function sendControl(patch: CrawlControlPatch): Promise<CrawlControlReply> {
  try {
    const resp = await fetch(url(), {
      method: 'POST',
      headers: headers(),
      body: JSON.stringify(patch),
      signal: AbortSignal.timeout(10_000),
    });
    const body = await resp.json().catch(() => null) as any;
    if (resp.ok) {
      const control = remember(body);
      if (control) return { ok: true, status: resp.status, control };
      return { ok: false, status: resp.status, error: 'sidecar respondeu corpo inválido' };
    }
    return {
      ok: false,
      status: resp.status,
      error: typeof body?.error === 'string' ? body.error : `sidecar respondeu HTTP ${resp.status}`,
    };
  } catch {
    return { ok: false, status: 0, error: 'sidecar não acessível' };
  }
}

/** Só para TESTE: fixa a foto (ou limpa com `null`). */
export function _setControlForTest(snapshot: CrawlControlSnapshot | null): void {
  last = snapshot;
}
