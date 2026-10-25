/**
 * MAIN-world interception. This source is bundled once as an unlisted WXT entry.
 * It observes successful page requests and MSE ownership, never starts playback.
 */
import { classifyMediaCandidate } from '../../../../shared/media-policy.mjs';

interface Evidence { url: string; protocol: 'direct' | 'hls' | 'dash'; blobUrl?: string }
export function installMediaHook(): void {
  const state = window as Window & { __dlman_media_hook__?: boolean };
  if (state.__dlman_media_hook__) return;
  state.__dlman_media_hook__ = true;
  const replay = new Map<string, Evidence>();
  const buffers = new WeakMap<object, Evidence>();
  const responseSources = new WeakMap<Response, Evidence>();
  const parents = new Map<string, Evidence>();
  const sourceBlobs = new WeakMap<MediaSource, string>();
  const sourceBuffers = new WeakMap<SourceBuffer, MediaSource>();
  let page = location.href;
  const emit = (evidence: Evidence) => {
    if (page !== location.href) { replay.clear(); parents.clear(); page = location.href; }
    const key = evidence.url + ':' + (evidence.blobUrl || '');
    if (replay.has(key)) return;
    replay.set(key, evidence);
    if (replay.size > 64) replay.delete(replay.keys().next().value!);
    window.postMessage({ source: 'dlman-media-hook', type: 'media-url-detected', ...evidence }, '*');
  };
  window.addEventListener('message', (event) => {
    if (event.source !== window || event.data?.source !== 'dlman-content' || event.data.type !== 'media-hook-ready') return;
    if (page !== location.href) { replay.clear(); parents.clear(); page = location.href; }
    for (const evidence of replay.values()) window.postMessage({ source: 'dlman-media-hook', type: 'media-url-detected', ...evidence }, '*');
  });
  const rememberPlaylist = (text: string, url: string, evidence: Evidence) => {
    if (text.length > 524288 || !text.includes('#EXTM3U')) return;
    for (const line of text.split(/\r?\n/)) {
      const value = line.trim();
      if (!value || value.startsWith('#')) continue;
      try {
        if (parents.size < 512) parents.set(new URL(value, url).href, evidence);
      } catch { /* Invalid playlist reference. */ }
    }
  };
  const readPlaylist = async (response: Response, url: string, evidence: Evidence) => {
    const reader = response.body?.getReader();
    if (!reader) return;
    const chunks: Uint8Array[] = [];
    let length = 0;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        length += value.byteLength;
        if (length > 524288) { void reader.cancel().catch(() => {}); return; }
        chunks.push(value);
      }
      const bytes = new Uint8Array(length);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      rememberPlaylist(new TextDecoder().decode(bytes), url, evidence);
    } finally { reader.releaseLock(); }
  };
  const originalFetch = window.fetch;
  window.fetch = function (...args: Parameters<typeof fetch>): Promise<Response> {
    return originalFetch.apply(this, args).then((response) => {
      try {
        if (!response.ok) return response;
        const input = args[0];
        const url = response.url || new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, location.href).href;
        const candidate = classifyMediaCandidate({ url, mime: response.headers.get('content-type') || undefined });
        const evidence = parents.get(url) || candidate;
        if (evidence) responseSources.set(response, evidence);
        if (candidate) {
          emit(candidate);
          if (candidate.protocol === 'hls') {
            void readPlaylist(response.clone(), url, evidence!).catch(() => {});
          }
        }
      } catch { /* Interception must not alter successful page fetches. */ }
      return response;
    });
  };
  // Track which complete stream supplied a buffer, rather than guessing which
  // player a request belongs to from timing, dimensions or DOM order.
  const originalArrayBuffer = Response.prototype.arrayBuffer;
  const originalClone = Response.prototype.clone;
  Response.prototype.arrayBuffer = function () {
    return originalArrayBuffer.call(this).then((buffer) => {
      const evidence = responseSources.get(this);
      if (evidence) buffers.set(buffer, evidence);
      return buffer;
    });
  };
  Response.prototype.clone = function () {
    const clone = originalClone.call(this), evidence = responseSources.get(this);
    if (evidence) responseSources.set(clone, evidence);
    return clone;
  };
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  const xhrUrls = new WeakMap<XMLHttpRequest, string>();
  XMLHttpRequest.prototype.open = function (method: string, url: string | URL, ...rest: any[]) {
    try { xhrUrls.set(this, new URL(String(url), location.href).href); } catch { /* Keep normal XHR behavior. */ }
    return originalOpen.apply(this, [method, url, ...rest] as any);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    this.addEventListener('load', () => {
      try {
        if (this.status < 200 || this.status >= 300) return;
        const url = this.responseURL || xhrUrls.get(this);
        if (!url) return;
        const candidate = classifyMediaCandidate({ url, mime: this.getResponseHeader('content-type') || undefined });
        const evidence = parents.get(url) || candidate;
        if (candidate) emit(candidate);
        if (evidence && this.responseType === 'arraybuffer' && this.response) buffers.set(this.response, evidence);
        if (candidate?.protocol === 'hls' && (!this.responseType || this.responseType === 'text')) {
          rememberPlaylist(this.responseText, url, evidence!);
        }
      } catch { /* Cross-origin or inaccessible response fields. */ }
    }, { once: true });
    return originalSend.apply(this, args);
  };
  if (typeof MediaSource !== 'undefined' && typeof SourceBuffer !== 'undefined') {
    const originalCreate = URL.createObjectURL;
    URL.createObjectURL = function (object) {
      const url = originalCreate.call(this, object);
      if (object instanceof MediaSource) sourceBlobs.set(object, url);
      return url;
    };
    const originalAdd = MediaSource.prototype.addSourceBuffer;
    MediaSource.prototype.addSourceBuffer = function (mime) {
      const buffer = originalAdd.call(this, mime);
      sourceBuffers.set(buffer, this);
      return buffer;
    };
    const originalAppend = SourceBuffer.prototype.appendBuffer;
    SourceBuffer.prototype.appendBuffer = function (data) {
      const result = originalAppend.call(this, data);
      const evidence = buffers.get(data) || (ArrayBuffer.isView(data) ? buffers.get(data.buffer) : undefined);
      const source = sourceBuffers.get(this), blobUrl = source && sourceBlobs.get(source);
      if (evidence && blobUrl) emit({ ...evidence, blobUrl });
      return result;
    };
  }
}
