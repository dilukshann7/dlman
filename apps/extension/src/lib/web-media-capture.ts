import { CAPTURE_TTL, captureSource, captureTabMatches, claimBrowserCapture, mergeCaptureSources, parseCaptureBridge, submitCapturedSources,
  type CaptureCandidate, type CaptureMetadata, type CaptureSource } from '../../../../shared/web-media-capture.mjs';
import { disabledSitesStorage, settingsStorage } from './storage';

const PREFIX = 'web-media-capture:';
const ALARM = 'web-media-capture-cleanup';
interface Ticket extends CaptureMetadata { phase: 'bridge' | 'watching' }
interface CaptureResponse { id: string; url: string; status: string }
interface Sender { id?: string; url?: string; frameId?: number; tab?: { id?: number } }
interface CaptureMessage { type: string; sources?: CaptureCandidate[] }
const tickets = new Map<number, Ticket>();
const sources = new Map<number, CaptureSource[]>();
const requestContext = new Map<string, { tab_id: number; referrer?: string; user_agent?: string }>();
let sequence = Promise.resolve();
let initialized: Promise<void> | null = null;

// Session storage survives an MV3 service-worker restart. Older Firefox versions use
// local storage for expiring ticket metadata only; media URLs/cookies never go there.
const metadataStorage = () => browser.storage.session ?? browser.storage.local;
const enqueue = <T>(operation: () => Promise<T>): Promise<T> => {
  const result = sequence.then(operation);
  sequence = result.then(() => undefined, () => undefined);
  return result;
};

async function request(ticket: Pick<Ticket, 'id' | 'port'>, action = '', payload?: unknown): Promise<CaptureResponse> {
  const response = await fetch(`http://127.0.0.1:${ticket.port}/api/web-media/capture/${ticket.id}${action ? `/${action}` : ''}`, {
    method: payload === undefined ? 'GET' : 'POST',
    headers: payload === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: payload === undefined ? undefined : JSON.stringify(payload),
    credentials: 'omit', cache: 'no-store', redirect: 'error', signal: AbortSignal.timeout(8000),
  });
  if (!response.ok) throw new Error('Browser capture is unavailable or expired.');
  return response.json() as Promise<CaptureResponse>;
}

async function allowed(url: string): Promise<boolean> {
  const settings = await settingsStorage.get();
  try { return settings.enabled && !await disabledSitesStorage.isDisabled(new URL(url).hostname); }
  catch { return false; }
}

async function save(ticket: Ticket): Promise<void> {
  tickets.set(ticket.tab_id, ticket);
  // Keep an explicit allowlist so credential-bearing media URLs cannot be persisted.
  const metadata: Ticket = { id: ticket.id, client_id: ticket.client_id, tab_id: ticket.tab_id,
    url: ticket.url, port: ticket.port, created: ticket.created, phase: ticket.phase };
  await metadataStorage().set({ [`${PREFIX}${ticket.id}`]: metadata });
}

async function clear(ticket: Ticket, cancel = true): Promise<void> {
  tickets.delete(ticket.tab_id);
  sources.delete(ticket.tab_id);
  for (const [id, context] of requestContext) if (context.tab_id === ticket.tab_id) requestContext.delete(id);
  await metadataStorage().remove(`${PREFIX}${ticket.id}`);
  if (cancel) await request(ticket, 'cancel', { client_id: ticket.client_id, tab_id: ticket.tab_id }).catch(() => {});
}

async function restore(): Promise<void> {
  const stored = await metadataStorage().get(null);
  for (const [key, value] of Object.entries(stored)) {
    if (!key.startsWith(PREFIX)) continue;
    const ticket = value as Ticket;
    if (!ticket || !Number.isInteger(ticket.tab_id) || !Number.isFinite(ticket.created)
      || ticket.created > Date.now() || Date.now() - ticket.created >= CAPTURE_TTL) {
      await metadataStorage().remove(key);
      continue;
    }
    tickets.set(ticket.tab_id, ticket);
  }
}

async function activeTicket(tabId: number): Promise<Ticket | null> {
  const ticket = tickets.get(tabId);
  if (!ticket) return null;
  const tab = await browser.tabs.get(tabId).catch(() => null);
  if (!tab || !captureTabMatches(ticket, tab) || !await allowed(ticket.url)) {
    await clear(ticket);
    return null;
  }
  // Recheck desktop cancellation before reading any cookies.
  const latest = await request(ticket).catch(() => null);
  if (!latest || !['watching', 'ready'].includes(latest.status) || latest.url !== ticket.url) {
    await clear(ticket, false);
    return null;
  }
  return ticket;
}

async function collect(tabId: number, candidates: CaptureCandidate[]): Promise<void> {
  const ticket = await activeTicket(tabId);
  if (!ticket) return;
  const previous = sources.get(tabId) || [];
  const merged = mergeCaptureSources(previous, candidates);
  // Signed queries identify distinct variants; never dedupe by pathname alone.
  const fresh = merged.filter((source) => !previous.some((old) => old.url === source.url));
  if (!fresh.length) return;
  try {
    const result = await submitCapturedSources(ticket, fresh, {
      getTab: (id) => browser.tabs.get(id), isAllowed: allowed,
      getCookies: (details) => browser.cookies.getAll(details),
      userAgent: navigator.userAgent,
      submit: (payload) => request(ticket, 'sources', payload),
    }) as CaptureResponse | null;
    if (result && ['watching', 'ready'].includes(result.status)) sources.set(tabId, merged);
    else if (result) await clear(ticket, false);
  } catch {
    // Keep payloads and signed URLs out of logs/errors. The desktop gives retry guidance.
    const tab = await browser.tabs.get(tabId).catch(() => null);
    if (!tab || !captureTabMatches(ticket, tab) || !await allowed(ticket.url)) await clear(ticket);
  }
}

/** Register synchronously with the background worker so restored tickets also work. */
export function setupWebMediaCapture(): void {
  initialized ??= restore();
  browser.alarms.create(ALARM, { periodInMinutes: 0.5 });
  browser.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name !== ALARM) return;
    void enqueue(async () => { await initialized; for (const ticket of [...tickets.values()]) await activeTicket(ticket.tab_id); });
  });
  browser.tabs.onRemoved.addListener((id) => {
    void enqueue(async () => { await initialized; const ticket = tickets.get(id); if (ticket) await clear(ticket); });
  });
  browser.tabs.onUpdated.addListener((id, info) => {
    if (!info.url) return;
    void enqueue(async () => {
      await initialized;
      const ticket = tickets.get(id);
      if (!ticket) return;
      if (ticket.phase === 'bridge' && parseCaptureBridge(info.url!, ticket.port)?.id === ticket.id) return;
      if (info.url !== ticket.url) await clear(ticket);
      else if (ticket.phase === 'bridge') await save({ ...ticket, phase: 'watching' });
    });
  });
  settingsStorage.watch(() => {
    void enqueue(async () => { await initialized; for (const ticket of [...tickets.values()]) if (!await allowed(ticket.url)) await clear(ticket); });
  });
  // Retain only the actual request Referer/User-Agent in memory; never Cookie headers.
  const rememberRequest: Parameters<typeof browser.webRequest.onBeforeSendHeaders.addListener>[0] = (details) => {
    if (!tickets.has(details.tabId)) return;
    const headerValue = (name: string) => details.requestHeaders?.find((header) => header.name.toLowerCase() === name)?.value;
    const context = { tab_id: details.tabId, referrer: headerValue('referer'), user_agent: headerValue('user-agent') };
    const requestId = details.requestId;
    void enqueue(async () => {
      await initialized;
      if (!tickets.has(context.tab_id) || requestContext.size >= 256) return;
      requestContext.set(requestId, context);
    });
  };
  try {
    browser.webRequest.onBeforeSendHeaders.addListener(rememberRequest, { urls: ['<all_urls>'] }, ['requestHeaders', 'extraHeaders']);
  } catch {
    // Firefox does not need Chromium's extraHeaders option to expose Referer.
    browser.webRequest.onBeforeSendHeaders.addListener(rememberRequest, { urls: ['<all_urls>'] }, ['requestHeaders']);
  }
  // onHeadersReceived confirms successful resource responses and captures manifests
  // from every frame inside the claimed tab without mixing unrelated tabs.
  browser.webRequest.onHeadersReceived.addListener((details) => {
    if (details.tabId < 0 || (details.statusCode !== 304 && (details.statusCode < 200 || details.statusCode >= 300))) return;
    const mime = details.responseHeaders?.find((header) => header.name.toLowerCase() === 'content-type')?.value;
    if (!captureSource({ url: details.url, mime })) return;
    void enqueue(async () => {
      await initialized;
      if (tickets.has(details.tabId)) {
        const candidate = captureSource({ url: details.url, mime, ...requestContext.get(details.requestId) });
        if (candidate) await collect(details.tabId, [candidate]);
      }
    });
  }, { urls: ['<all_urls>'] }, ['responseHeaders']);
  const releaseRequest = (details: { requestId: string }) => {
    void enqueue(async () => { requestContext.delete(details.requestId); });
  };
  browser.webRequest.onCompleted.addListener(releaseRequest, { urls: ['<all_urls>'] });
  browser.webRequest.onErrorOccurred.addListener(releaseRequest, { urls: ['<all_urls>'] });
}

export async function handleWebMediaCaptureMessage(message: CaptureMessage, sender: Sender): Promise<unknown> {
  return enqueue(async () => {
    await initialized;
    if (sender.id !== browser.runtime.id || sender.tab?.id == null) throw new Error('Invalid capture tab.');
    const tabId = sender.tab.id;
    if (message.type === 'web-media-capture-claim') {
      const settings = await settingsStorage.get();
      const tab = await browser.tabs.get(tabId);
      const bridge = parseCaptureBridge(tab.url || '', settings.port);
      if (!settings.enabled || sender.frameId !== 0 || sender.url !== tab.url || tab.incognito || !bridge
        || !await allowed(tab.url!)) throw new Error('Invalid capture bridge.');
      const current = tickets.get(tabId);
      if (current?.id === bridge.id) return { success: true };
      if (tickets.size >= 16) throw new Error('Too many pending captures.');
      if (current) await clear(current);
      const metadata = await claimBrowserCapture(sender, {
        extensionId: browser.runtime.id, port: settings.port,
        getTab: (id) => browser.tabs.get(id), isAllowed: allowed,
        newClientId: () => crypto.randomUUID(),
        claim: (target, payload) => request(target, 'claim', payload),
        cancel: (target, payload) => request(target, 'cancel', payload),
      });
      const ticket: Ticket = { ...metadata, phase: 'bridge' };
      await save(ticket);
      try { await browser.tabs.update(tabId, { url: ticket.url, active: true }); }
      catch { await clear(ticket); throw new Error('Could not open capture page.'); }
      return { success: true };
    }
    const ticket = await activeTicket(tabId);
    if (!ticket) return { active: false };
    if (message.type === 'web-media-capture-sources' && Array.isArray(message.sources) && message.sources.length <= 32) {
      // sender.url is supplied by the browser for this frame; page code cannot set it.
      const referrer = sender.url;
      if (referrer && await allowed(referrer)) await collect(tabId, message.sources.map((candidate) => ({
        url: candidate.url, mime: candidate.mime, protocol: candidate.protocol, title: candidate.title, referrer,
      })));
    }
    return { active: true };
  });
}
