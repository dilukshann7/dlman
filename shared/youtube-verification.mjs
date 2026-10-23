import { canonicalYoutubeUrl } from './youtube.mjs';

const TOKEN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const DOMAINS = new Set(['youtube.com', '.youtube.com', 'www.youtube.com']);
const MAX_AGE = 10 * 60 * 1000;

export function parseYoutubeVerificationUrl(value) {
  try {
    const parsed = new URL(value);
    const url = canonicalYoutubeUrl(value);
    const token = /^#dlman-verify=([^&]+)$/.exec(parsed.hash)?.[1];
    if (parsed.protocol !== 'https:' || parsed.hostname !== 'www.youtube.com' || !url || !TOKEN.test(token || '')) return null;
    return { token, url };
  } catch { return null; }
}

/** Copy only the fields needed by the scoped Netscape cookie jar. Never log the result. */
export function youtubeSessionCookies(cookies, now = Date.now()) {
  if (!Array.isArray(cookies) || cookies.length > 256) throw new Error('Too many YouTube cookies.');
  const result = [];
  for (const cookie of cookies) {
    if (!DOMAINS.has(cookie.domain)) continue;
    if (cookie.expirationDate != null && cookie.expirationDate * 1000 <= now) continue;
    if (![cookie.name, cookie.value, cookie.path].every((value) => typeof value === 'string' && !/[\x00-\x20\x7f]/.test(value))
      || !cookie.name || cookie.name.length > 1024 || cookie.value.length > 8192 || !cookie.path.startsWith('/')
      || (cookie.expirationDate != null && (!Number.isFinite(cookie.expirationDate) || cookie.expirationDate < 0))) {
      throw new Error('Invalid YouTube cookie.');
    }
    result.push({ domain: cookie.domain, path: cookie.path, name: cookie.name, value: cookie.value,
      secure: !!cookie.secure, http_only: !!cookie.httpOnly, host_only: !!cookie.hostOnly,
      expires: cookie.expirationDate == null ? null : Math.floor(cookie.expirationDate) });
  }
  if (!result.length) throw new Error('No YouTube session cookies were found. Open the video and complete verification first.');
  if (new TextEncoder().encode(JSON.stringify(result)).length > 64 * 1024) throw new Error('YouTube session is too large.');
  return result;
}

/** Called only after approval in an extension-owned page; recheck navigation around cookie access. */
export async function shareYoutubeSession(request, dependencies) {
  const now = dependencies.now || Date.now;
  const checkAge = () => {
    if (!TOKEN.test(request.token) || !Number.isFinite(request.created) || now() - request.created > MAX_AGE || now() < request.created) {
      throw new Error('Verification expired. Start again in DLMan.');
    }
  };
  const checkTab = (tab) => {
    if (tab.incognito) throw new Error('YouTube verification is not supported in private browsing.');
    const verification = parseYoutubeVerificationUrl(tab.url || '');
    if (!request.url || verification?.url !== request.url || verification.token !== request.token) {
      throw new Error('Video changed. Start verification again in DLMan.');
    }
  };
  checkAge();
  const tab = await dependencies.getTab(request.tabId);
  checkTab(tab);
  const cookies = youtubeSessionCookies(await dependencies.getCookies({ url: request.url,
    ...(tab.cookieStoreId ? { storeId: tab.cookieStoreId } : {}) }), now());
  const latest = await dependencies.getTab(request.tabId);
  checkTab(latest);
  if (latest.cookieStoreId !== tab.cookieStoreId) throw new Error('Browser session changed. Start verification again.');
  checkAge();
  return dependencies.submit({ token: request.token, url: request.url, cookies });
}
