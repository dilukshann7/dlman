import { defineContentScript } from 'wxt/sandbox';
import { parseCaptureBridge } from '../../../../shared/web-media-capture.mjs';
import { collectPlayerSources } from '../../../../shared/player-sources.mjs';
import { snapshotPlayers } from '../lib/player-dom';

/** Passive observer; the desktop's explicit bridge claim enables this tab only. */
export default defineContentScript({
  matches: ['<all_urls>'], allFrames: true, matchAboutBlank: true, runAt: 'document_start',
  async main() {
    const local = new URL(location.href);
    if (local.hostname === '127.0.0.1' && local.pathname === '/web-media-capture') {
      if (window.top === window && parseCaptureBridge(location.href, Number(local.port || '80'))) {
        void browser.runtime.sendMessage({ type: 'web-media-capture-claim' }).catch(() => {});
      }
      return;
    }
    const response = await browser.runtime.sendMessage({ type: 'web-media-capture-active' }).catch(() => null) as { active?: boolean } | null;
    if (!response?.active) return;
    const seen = new Set<string>();
    const identities = new WeakMap<HTMLElement, string>();
    let nextId = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const scan = () => {
      timer = undefined;
      const data = snapshotPlayers((element) => {
        let id = identities.get(element);
        if (!id) { id = 'capture-' + ++nextId; identities.set(element, id); }
        return id;
      });
      for (const root of data.roots) observer.observe(root, { childList: true, subtree: true,
        characterData: true, attributes: true, attributeFilter: ['src', 'type', 'label', 'title', 'data-setup', 'class'] });
      const candidates = collectPlayerSources(data.snapshot).filter((source) => !seen.has(source.url));
      if (!candidates.length) return;
      candidates.forEach((source) => seen.add(source.url));
      void browser.runtime.sendMessage({ type: 'web-media-capture-sources', sources: candidates }).catch(() => {
        candidates.forEach((source) => seen.delete(source.url));
      });
    };
    const schedule = () => { timer ??= setTimeout(scan, 150); };
    document.addEventListener('playing', schedule, true);
    document.addEventListener('loadedmetadata', schedule, true);
    document.addEventListener('canplay', schedule, true);
    const observer = new MutationObserver(schedule);
    observer.observe(document, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['src', 'type', 'label', 'title'] });
    window.addEventListener('pagehide', () => { observer.disconnect(); if (timer) clearTimeout(timer); }, { once: true });
    schedule();
  },
});
