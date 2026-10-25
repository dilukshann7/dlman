import type { PlayerSnapshot, PlayerVideo } from '../../../../shared/player-sources.mjs';
import { isAdResource } from '../../../../shared/media-policy.mjs';

const PLAYER = '.video-js,.jwplayer,.plyr,.flowplayer,[data-player-id]';
const AD = '[data-ad="true"],[data-is-ad="true"],.ad-container,.video-ad,.vast-player,.vpaid-player';
const AD_PLAYING = '.vjs-ad-playing,.jw-flag-ads,.ad-showing,[data-ad-playing="true"]';
export interface PlayerDOMSnapshot {
  snapshot: PlayerSnapshot;
  targets: Map<string, HTMLElement>;
  videos: Map<string, HTMLVideoElement[]>;
  roots: Array<Document | ShadowRoot>;
  adPlaying: Set<string>;
}

/** Enumerate accessible shadow roots. Closed roots are intentionally left to native extraction. */
export function mediaRoots(): Array<Document | ShadowRoot> {
  const roots: Array<Document | ShadowRoot> = [document];
  for (let i = 0; i < roots.length && roots.length < 32; i++) {
    for (const element of roots[i].querySelectorAll('*')) {
      if (element.shadowRoot && roots.length < 32) roots.push(element.shadowRoot);
    }
  }
  return roots;
}

export function playerVisible(target: HTMLElement): boolean {
  const rect = target.getBoundingClientRect();
  if (!target.isConnected || rect.width < 120 || rect.height < 68 || rect.bottom <= 0 || rect.right <= 0
    || rect.top >= innerHeight || rect.left >= innerWidth) return false;
  const style = getComputedStyle(target);
  return style.display !== 'none' && style.visibility !== 'hidden' && style.opacity !== '0';
}

/** Ownership follows a player's DOM root, never the largest or first video in the document. */
export function snapshotPlayers(identity: (element: HTMLElement) => string): PlayerDOMSnapshot {
  const roots = mediaRoots(), targets = new Map<string, HTMLElement>(), videos = new Map<string, HTMLVideoElement[]>();
  const entries = new Map<string, PlayerVideo>(), adPlaying = new Set<string>();
  for (const root of roots) {
    const elements = [...root.querySelectorAll<HTMLVideoElement>('video')];
    const placeholders = [...root.querySelectorAll<HTMLElement>(PLAYER)]
      .filter((element) => !element.querySelector('video') && (element.id || element.hasAttribute('data-setup')));
    for (const element of [...elements, ...placeholders].slice(0, 64)) {
      if (element.closest(AD)) continue;
      const target = element.closest<HTMLElement>(PLAYER) || element;
      let id = target.id || element.id || identity(target);
      if (targets.has(id) && targets.get(id) !== target) id = identity(target);
      targets.set(id, target);
      const entry = entries.get(id) || { id, configId: target.id || element.id || id,
        sources: [], setup: target.getAttribute('data-setup') || undefined };
      if (element.tagName === 'VIDEO') {
        const video = element as HTMLVideoElement;
        videos.set(id, [...(videos.get(id) || []), video]);
        const isAd = !!video.closest(AD_PLAYING) || isAdResource(video.currentSrc || video.src);
        if (isAd) adPlaying.add(id);
        if (!isAd) {
          entry.currentSrc ||= video.currentSrc || video.src;
          entry.src ||= video.src;
          entry.sources!.push(...[...video.querySelectorAll('source')].slice(0, 32).map((source) => ({
            src: source.src, type: source.type, label: source.getAttribute('label') || source.getAttribute('title') || undefined,
            fps: Number(source.getAttribute('data-fps')) || undefined,
          })));
        }
        entry.setup ||= video.getAttribute('data-setup') || undefined;
      }
      entries.set(id, entry);
    }
  }
  const scripts = roots.flatMap((root) => [...root.querySelectorAll<HTMLScriptElement>('script:not([src])')])
    .slice(0, 128).map((script) => ({ type: script.type, text: script.textContent || '' }));
  return { targets, videos, roots, adPlaying, snapshot: {
    baseUrl: document.baseURI, title: document.title, videos: [...entries.values()].slice(0, 32), scripts,
  } };
}
