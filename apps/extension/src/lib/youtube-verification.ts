import { parseYoutubeVerificationUrl, shareYoutubeSession, type YoutubeVerificationRequest } from '../../../../shared/youtube-verification.mjs';
import { getDlmanClient } from './api-client';

const STORAGE_PREFIX = 'youtube-verification:';
const TTL = 10 * 60 * 1000;
type Requests = Record<string, YoutubeVerificationRequest>;
interface Sender { id?: string; url?: string; frameId?: number; tab?: { id?: number } }

async function readRequests(): Promise<Requests> {
  const stored = await browser.storage.local.get(null);
  const requests: Requests = {};
  const expired: string[] = [];
  for (const [key, value] of Object.entries(stored)) {
    if (!key.startsWith(STORAGE_PREFIX)) continue;
    const request = value as YoutubeVerificationRequest;
    if (request && Number.isFinite(request.created) && Date.now() - request.created < TTL) requests[key.slice(STORAGE_PREFIX.length)] = request;
    else expired.push(key);
  }
  if (expired.length) await browser.storage.local.remove(expired);
  return requests;
}

/** No cookies are read until approval comes from our extension-owned window. */
export async function handleYoutubeVerificationMessage(message: { type: string; token?: string }, sender: Sender) {
  const requests = await readRequests();
  const token = message.token || '';
  if (message.type === 'youtube-verification-request') {
    if (sender.tab?.id == null || (sender.frameId != null && sender.frameId !== 0)) throw new Error('Open verification in a normal YouTube tab.');
    const tab = await browser.tabs.get(sender.tab.id);
    const verification = parseYoutubeVerificationUrl(tab.url || '');
    if (!verification || verification.token !== token || tab.incognito) throw new Error('Invalid YouTube verification tab.');
    if (requests[token]) return { success: true };
    if (Object.keys(requests).length >= 16) throw new Error('Too many pending browser verifications.');
    requests[token] = { ...verification, tabId: sender.tab.id, created: Date.now() };
    await browser.storage.local.set({ [`${STORAGE_PREFIX}${token}`]: requests[token] });
    const page = new URL('youtube-verify.html', browser.runtime.getURL('/')).href;
    await browser.windows.create({ url: `${page}#${token}`, type: 'popup', width: 500, height: 440 });
    return { success: true };
  }

  const approvalPage = new URL('youtube-verify.html', browser.runtime.getURL('/')).href;
  if (sender.id !== browser.runtime.id || sender.url?.split(/[?#]/)[0] !== approvalPage) throw new Error('Approve session sharing in the DLMan extension window.');
  const request = Object.hasOwn(requests, token) ? requests[token] : undefined;
  if (!request) throw new Error('Verification expired. Start again in DLMan.');
  if (message.type === 'youtube-verification-details') return { success: true, url: request.url };
  if (message.type === 'youtube-verification-approve') {
    const response = await shareYoutubeSession(request, {
      getTab: (id) => browser.tabs.get(id),
      getCookies: (details) => browser.cookies.getAll(details),
      submit: (payload) => getDlmanClient().submitYoutubeVerification(payload),
    });
    await browser.storage.local.remove(`${STORAGE_PREFIX}${token}`);
    return response;
  }
  if (message.type === 'youtube-verification-cancel') {
    await getDlmanClient().cancelYoutubeVerification(token);
    await browser.storage.local.remove(`${STORAGE_PREFIX}${token}`);
    return { success: true };
  }
  throw new Error('Unknown browser verification action.');
}

export function offerYoutubeVerification(): void {
  const verification = parseYoutubeVerificationUrl(location.href);
  if (!verification) return;
  void browser.runtime.sendMessage({ type: 'youtube-verification-request', token: verification.token }).catch(() => {
    // The desktop provides timeout and retry guidance; never log the verification URL or token.
  });
}
