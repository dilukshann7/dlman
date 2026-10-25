import { AutomaticInspectionQueue } from '../../../../shared/media-discovery.mjs';
import { publicMediaUrl, isAdResource } from '../../../../shared/media-policy.mjs';
import { isYoutubeHost } from '../../../../shared/youtube.mjs';
import { settingsStorage, disabledSitesStorage } from './storage';
import { getDlmanClient } from './api-client';

interface Sender { id?: string; url?: string; frameId?: number; documentId?: string; tab?: { id?: number; url?: string } }
interface Message { type: string; url?: string; document_id?: string; player_id?: string; retry?: boolean; eligible?: boolean }
const scheduler = new AutomaticInspectionQueue({ poll: (id) => getDlmanClient().webMediaInspection(id) });
const documents = new Map<string, string>();
const players = new Map<string, Map<string, boolean>>();

export function forgetAutomaticMediaTab(tabId: number): void {
  for (const key of documents.keys()) if (key.startsWith(tabId + ':')) documents.delete(key);
  for (const key of players.keys()) if (key.startsWith(tabId + ':')) players.delete(key);
}
export function resetAutomaticMedia(): void { scheduler.clear(); }

export async function handleAutomaticMedia(message: Message, sender: Sender): Promise<unknown> {
  if (sender.id !== browser.runtime.id || sender.tab?.id == null || sender.frameId == null
    || typeof message.document_id !== 'string' || !/^[a-z0-9-]{1,64}$/i.test(message.document_id)) {
    throw new Error('Invalid video document.');
  }
  const tabId = sender.tab.id, frameKey = tabId + ':' + sender.frameId;
  const documentKey = (sender.documentId || '') + ':' + message.document_id;
  if (message.type === 'media-discovery-ready') {
    if (documents.get(frameKey) !== documentKey) players.delete(frameKey);
    documents.set(frameKey, documentKey);
    const settings = await settingsStorage.get();
    const tab = await browser.tabs.get(tabId);
    const page = publicMediaUrl(sender.url);
    const enabled = !!page && settings.enabled && !await disabledSitesStorage.isDisabled(page.hostname)
      && (!tab.url || !await disabledSitesStorage.isDisabled(new URL(tab.url).hostname));
    return { ok: true, enabled };
  }
  if (message.type === 'media-discovery-stop') {
    if (documents.get(frameKey) === documentKey) { documents.delete(frameKey); players.delete(frameKey); }
    return { ok: true };
  }
  if (message.type === 'media-discovery-player') {
    if (documents.get(frameKey) !== documentKey || typeof message.player_id !== 'string'
      || message.player_id.length > 256 || typeof message.eligible !== 'boolean') return { ok: false };
    const known = players.get(frameKey) || new Map<string, boolean>();
    known.set(message.player_id, message.eligible);
    if (known.size > 128) {
      for (const [id, eligible] of known) if (!eligible && id !== message.player_id) known.delete(id);
    }
    players.set(frameKey, known);
    return { ok: true };
  }
  const page = publicMediaUrl(sender.url);
  if (!page || isYoutubeHost(page.href) || isAdResource(page.href) || message.url !== page.href
    || typeof message.player_id !== 'string' || message.player_id.length > 256) throw new Error('Invalid video page.');
  // A suspended MV3 worker may lose registrations. The browser-supplied sender
  // still identifies this content-script document on its first request after wake.
  if (!documents.has(frameKey)) documents.set(frameKey, documentKey);
  const knownPlayers = players.get(frameKey) || new Map<string, boolean>();
  // Restore eligibility from a still-visible content-script request after MV3
  // suspension, but never revive a player explicitly cancelled in this worker.
  if (!knownPlayers.has(message.player_id)) knownPlayers.set(message.player_id, true);
  players.set(frameKey, knownPlayers);
  const topUrl = sender.tab.url;
  const isCurrent = () => documents.get(frameKey) === documentKey && players.get(frameKey)?.get(message.player_id!) === true;
  const authorize = async () => {
    const tab = await browser.tabs.get(tabId);
    const settings = await settingsStorage.get();
    if (!tab.active) throw Object.assign(new Error('Open this video tab to detect its sources.'), { deferred: true });
    if (!isCurrent() || tab.url !== topUrl || !settings.enabled
      || await disabledSitesStorage.isDisabled(page.hostname)
      || tab.url && await disabledSitesStorage.isDisabled(new URL(tab.url).hostname)) throw new Error('Video page changed or detection is disabled.');
    return settings;
  };
  const key = frameKey + ':' + documentKey + ':' + message.player_id + ':' + page.href;
  await authorize();
  return scheduler.request({ key, isCurrent, start: async () => {
    const settings = await authorize();
    const client = getDlmanClient({ port: settings.port });
    try { return await client.inspectWebMedia(page.href); }
    catch (error) {
      if ((error as { status?: number })?.status === 429) return { id: 'queued', status: 'pending' as const };
      throw error;
    }
  } }, message.retry);
}
