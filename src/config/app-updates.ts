import { DEFAULT_APP_UPDATES_DIR } from './helpers.js';

// Fábrica (ver server.ts): relê o process.env a cada montagem do config.
// Atualização do app Power Movie servida por esta instância. Sem token a
// rota fica desligada (404): o APK leva as chaves compiladas do app, então o
// download nunca é público.
export const appUpdates = () => ({
  token: (process.env.APP_UPDATE_TOKEN || '').trim(),
  dir: process.env.APP_UPDATE_DIR || DEFAULT_APP_UPDATES_DIR,
});

// FlareSolverr do container (o mesmo que o Jackett usa): resolve os códigos da
// AFTVnews, que passou a exigir desafio do Cloudflare.
export const flaresolverrUrl = () => process.env.FLARESOLVERR_URL || 'http://127.0.0.1:8191/v1';

const intEnv = (name: string, fallback: number, min: number) => {
  const value = Number(process.env[name]);
  return Number.isFinite(value) && value >= min ? Math.floor(value) : fallback;
};

// Teto da ida à AFTVnews no /tv-code (janela de 10 min). O FlareSolverr é
// serial e compartilhado com os indexers BR e o raspador: código aleatório
// sem teto próprio segurava o solver e degradava a busca de todos.
export const tvCodeAftv = () => ({
  perIp: intEnv('TV_CODE_AFTV_PER_IP', 4, 1),
  global: intEnv('TV_CODE_AFTV_GLOBAL', 30, 1),
  concurrent: intEnv('TV_CODE_AFTV_CONCURRENT', 1, 1),
});
