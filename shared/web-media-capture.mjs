import { classifyMediaCandidate, publicMediaUrl } from './media-policy.mjs';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
export const CAPTURE_TTL = 10 * 60 * 1000;
const MAX_SOURCES = 32;

/** Only the bridge opened by the desktop can claim a capture. Never forward its fragment. */
export function parseCaptureBridge(value, expectedPort) {
  try {
    const parsed = new URL(value);
    const id = parsed.hash.slice(1);
    if (parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1' || Number(parsed.port || '80') !== expectedPort
      || parsed.pathname !== '/web-media-capture' || parsed.search || parsed.username || parsed.password || !UUID.test(id)) return null;
    return { id, port: expectedPort };
  } catch { return null; }
}

function publicHttpUrl(value) {
  return publicMediaUrl(value);
}

/** Browser-supplied sender/tab data, never a page-provided URL, authorizes a claim. */
export async function claimBrowserCapture(sender, dependencies) {
  if (sender.id !== dependencies.extensionId || sender.frameId !== 0 || sender.tab?.id == null) throw new Error('Invalid capture tab.');
  const tab = await dependencies.getTab(sender.tab.id);
  const bridge = parseCaptureBridge(tab.url || '', dependencies.port);
  if (!bridge || sender.url !== tab.url || tab.incognito || tab.id !== sender.tab.id || !await dependencies.isAllowed(tab.url)) throw new Error('Invalid capture bridge.');
  const claim = { client_id: dependencies.newClientId(), tab_id: tab.id };
  if (typeof claim.client_id !== 'string' || !/^[a-z0-9_-]{1,128}$/i.test(claim.client_id)) throw new Error('Invalid capture client identifier.');
  const capture = await dependencies.claim(bridge, claim);
  const metadata = { ...bridge, ...claim, url: capture.url, created: (dependencies.now || Date.now)() };
  const latest = await dependencies.getTab(tab.id);
  if (capture.id !== bridge.id || !['watching', 'ready'].includes(capture.status) || latest.url !== tab.url || latest.incognito
    || latest.id !== tab.id || !publicHttpUrl(capture.url) || !await dependencies.isAllowed(capture.url)) {
    await dependencies.cancel(bridge, claim).catch(() => {});
    throw new Error('Capture page changed or is unavailable.');
  }
  return metadata;
}

export function captureTabMatches(request, tab, now = Date.now()) {
  return !!(UUID.test(request.id || '') && Number.isFinite(request.created) && now >= request.created
    && now - request.created < CAPTURE_TTL && request.tab_id === tab.id && !tab.incognito
    && publicHttpUrl(request.url) && tab.url === request.url);
}

/** Accept manifest/direct video resources, never transport segments or arbitrary page messages. */
export function captureSource(candidate) {
  const classified = classifyMediaCandidate(candidate);
  if (!classified) return null;
  // KVS and HTML5 players commonly serve /get_file/.../video.mp4/. Inspect the
  // suffix without rewriting the actual request path or signed query string.
  return { ...classified,
    ...(typeof candidate.title === 'string' ? { title: candidate.title.replace(/[\x00-\x1f\x7f]/g, '').slice(0, 512) } : {}),
    ...(typeof candidate.user_agent === 'string' && candidate.user_agent.length <= 1024 && !/[\x00-\x1f\x7f]/.test(candidate.user_agent) ? { user_agent: candidate.user_agent } : {}),
    ...(typeof candidate.referrer === 'string' && publicHttpUrl(candidate.referrer) ? { referrer: candidate.referrer } : {}) };
}

export function mergeCaptureSources(existing, candidates) {
  const result = new Map();
  for (const candidate of [...existing, ...candidates]) {
    // Validated existing entries retain their MIME-independent protocol for extensionless URLs.
    const mime = candidate.protocol === 'hls' ? 'application/vnd.apple.mpegurl'
      : candidate.protocol === 'dash' ? 'application/dash+xml' : candidate.protocol === 'direct' ? 'video/mp4' : candidate.mime;
    const source = captureSource({ ...candidate, mime });
    if (source && (result.has(source.url) || result.size < MAX_SOURCES)) result.set(source.url, source);
  }
  return [...result.values()];
}

/** Cookie API URL filtering is repeated here to enforce domain, secure and path boundaries. */
export function scopedMediaCookies(cookies, url, now = Date.now()) {
  const parsed = publicHttpUrl(url);
  if (!parsed || !Array.isArray(cookies) || cookies.length > 256) throw new Error('Invalid media cookies.');
  const result = [];
  for (const cookie of cookies) {
    if (typeof cookie.domain !== 'string' || typeof cookie.path !== 'string') continue;
    const domain = cookie.domain.replace(/^\./, '').toLowerCase();
    const matchesHost = cookie.hostOnly ? parsed.hostname === domain : parsed.hostname === domain || parsed.hostname.endsWith(`.${domain}`);
    const matchesPath = parsed.pathname === cookie.path || parsed.pathname.startsWith(cookie.path.endsWith('/') ? cookie.path : `${cookie.path}/`);
    if (!matchesHost || !matchesPath || !cookie.path.startsWith('/') || (cookie.secure && parsed.protocol !== 'https:')
      || (cookie.expirationDate != null && cookie.expirationDate * 1000 <= now)) continue;
    if (![cookie.name, cookie.value, cookie.path].every((value) => typeof value === 'string' && !/[\x00-\x20\x7f]/.test(value))
      || !cookie.name || cookie.name.length > 1024 || cookie.value.length > 8192
      || (cookie.expirationDate != null && (!Number.isFinite(cookie.expirationDate) || cookie.expirationDate < 0))) throw new Error('Invalid media cookie.');
    result.push({ domain: cookie.domain, path: cookie.path, name: cookie.name, value: cookie.value,
      secure: !!cookie.secure, http_only: !!cookie.httpOnly, host_only: !!cookie.hostOnly,
      expires: cookie.expirationDate == null ? null : Math.floor(cookie.expirationDate) });
  }
  if (new TextEncoder().encode(JSON.stringify(result)).length > 64 * 1024) throw new Error('Media cookies are too large.');
  return result;
}

/** Credentials stay inside this call; callers persist only ticket metadata. Recheck after each await. */
export async function submitCapturedSources(request, candidates, dependencies) {
  const now = dependencies.now || Date.now;
  const checkTab = async () => {
    const tab = await dependencies.getTab(request.tab_id);
    if (!captureTabMatches(request, tab, now()) || !await dependencies.isAllowed(request.url)) throw new Error('Browser capture expired or page changed.');
    return tab;
  };
  const tab = await checkTab();
  const sources = [];
  for (const source of mergeCaptureSources([], candidates)) {
    if (!await dependencies.isAllowed(source.url) || (source.referrer && !await dependencies.isAllowed(source.referrer))) continue;
    const cookies = scopedMediaCookies(await dependencies.getCookies({ url: source.url,
      ...(tab.cookieStoreId ? { storeId: tab.cookieStoreId } : {}) }), source.url, now());
    sources.push({ ...source, referrer: source.referrer || request.url, user_agent: source.user_agent || dependencies.userAgent, cookies });
  }
  const latest = await checkTab();
  if (latest.cookieStoreId !== tab.cookieStoreId) throw new Error('Browser session changed.');
  for (const source of sources) {
    if (!await dependencies.isAllowed(source.url) || !await dependencies.isAllowed(source.referrer)) throw new Error('Browser capture source was disabled.');
  }
  if (!sources.length) return null;
  const payload = { client_id: request.client_id, tab_id: request.tab_id, page_url: request.url, sources };
  if (new TextEncoder().encode(JSON.stringify(payload)).length > 256 * 1024) throw new Error('Browser capture is too large.');
  return dependencies.submit(payload);
}
