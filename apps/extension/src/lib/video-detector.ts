/**
 * Detect complete video sources before playback and retain explicit player ownership.
 * Network hints confirm known sources; ambiguous requests never attach by screen size.
 */
import { collectPlayerGroups } from '../../../../shared/player-sources.mjs';
import { classifyMediaCandidate, publicMediaUrl } from '../../../../shared/media-policy.mjs';
import type { DetectedMedia, MediaProtocol } from './media-types';
import { snapshotPlayers, playerVisible } from './player-dom';

export type OnMediaDetected = (media: DetectedMedia, target?: HTMLElement) => void;
export interface VideoDetectorOptions {
  onDetected: OnMediaDetected;
  onRemoved?: (id: string) => void;
  onVisibility?: (id: string, visible: boolean) => void;
  /** Kept for callers compiled against the previous detector; short content is valid. */
  minDuration?: number;
  observeDOM?: boolean;
}
export class VideoDetector {
  readonly detected = new Map<string, DetectedMedia>();
  private targets = new Map<string, HTMLElement>();
  private identities = new WeakMap<HTMLElement, string>();
  private playerVideos = new WeakMap<HTMLElement, HTMLVideoElement | undefined>();
  private signatures = new Map<string, string>();
  private observers = new Map<Document | ShadowRoot, MutationObserver>();
  private cleanups: Array<() => void> = [];
  private resize?: ResizeObserver;
  private visibility?: IntersectionObserver;
  private observed = new WeakSet<HTMLElement>();
  private intersections = new WeakMap<HTMLElement, boolean>();
  private nextId = 0;
  private epoch = 0;
  private page = location.href;
  private timer?: ReturnType<typeof setTimeout>;
  private dead = false;
  private enabled = true;
  private network = new Map<string, { url: string; protocol: MediaProtocol; blobUrl?: string }>();
  constructor(private opts: VideoDetectorOptions) {}

  start(): void {
    if (this.dead) return;
    this.resize = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(() => this.schedule());
    this.visibility = typeof IntersectionObserver === 'undefined' ? undefined : new IntersectionObserver((entries) => {
      for (const entry of entries) this.intersections.set(entry.target as HTMLElement, entry.isIntersecting);
      this.schedule();
    });
    for (const event of ['loadedmetadata', 'loadstart', 'canplay', 'playing', 'emptied', 'visibilitychange']) {
      document.addEventListener(event, this.schedule, true);
      this.cleanups.push(() => document.removeEventListener(event, this.schedule, true));
    }
    for (const event of ['popstate', 'hashchange', 'resize', 'scroll']) {
      window.addEventListener(event, this.schedule, { passive: true });
      this.cleanups.push(() => window.removeEventListener(event, this.schedule));
    }
    // Also catches pushState navigation and newly attached open shadow roots.
    const interval = setInterval(this.schedule, 1000);
    this.cleanups.push(() => clearInterval(interval));
    this.scan();
  }

  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    if (!enabled) this.clear();
    else this.schedule();
  }

  retryUnresolved(onlyDeferred = false): void {
    for (const [id, media] of this.detected) {
      if (media.protocol === 'webpage' && (onlyDeferred ? media.discovery_status === 'deferred' : media.discovery_status !== 'ready')) this.signatures.delete(id);
    }
    this.schedule();
  }

  isVisible(id: string): boolean {
    const target = this.targets.get(id);
    return !!target && playerVisible(target) && this.intersections.get(target) !== false;
  }

  private clear(): void {
    for (const id of this.detected.keys()) this.opts.onRemoved?.(id);
    this.detected.clear(); this.signatures.clear(); this.targets.clear(); this.network.clear();
  }

  destroy(): void {
    this.dead = true;
    if (this.timer) clearTimeout(this.timer);
    for (const observer of this.observers.values()) observer.disconnect();
    this.observers.clear(); this.resize?.disconnect(); this.visibility?.disconnect();
    this.cleanups.forEach((cleanup) => cleanup()); this.cleanups = []; this.clear();
  }

  private schedule = (): void => {
    if (!this.dead && !this.timer) this.timer = setTimeout(() => { this.timer = undefined; this.scan(); }, 100);
  };

  /** The callback result is accepted only while the original player candidate is current. */
  resolvePlayer(original: DetectedMedia, resolved: DetectedMedia): boolean {
    if (this.dead || !this.enabled || this.detected.get(original.id) !== original
      || !this.targets.get(original.id)?.isConnected || location.href !== this.page) return false;
    this.detected.set(original.id, resolved);
    this.opts.onDetected(resolved, this.targets.get(original.id));
    return true;
  }

  /** MIME hints may classify extensionless sources, but must still match this player's source/config. */
  injectStreamUrl(url: string, protocol?: MediaProtocol, blobUrl?: string): void {
    const mime = protocol === 'hls' ? 'application/vnd.apple.mpegurl'
      : protocol === 'dash' ? 'application/dash+xml' : 'video/mp4';
    const candidate = classifyMediaCandidate({ url, mime });
    if (!candidate || blobUrl && !blobUrl.startsWith('blob:')) return;
    const key = blobUrl || candidate.url;
    this.network.set(key, { ...candidate, blobUrl });
    if (this.network.size > 64) this.network.delete(this.network.keys().next().value!);
    this.schedule();
  }

  private scan(): void {
    if (this.dead || !this.enabled) return;
    if (this.page !== location.href) { this.clear(); this.page = location.href; this.epoch++; }
    const data = snapshotPlayers((element) => {
      let id = this.identities.get(element);
      if (!id) { id = 'player-' + ++this.nextId; this.identities.set(element, id); }
      return id;
    });
    for (const root of data.roots) {
      if (this.opts.observeDOM === false || this.observers.has(root)) continue;
      const observer = new MutationObserver((mutations) => {
        if (mutations.some((mutation) => !(mutation.target instanceof Element
          && mutation.target.closest('dlman-overlay')))) this.schedule();
      });
      observer.observe(root, { subtree: true, childList: true, characterData: true, attributes: true,
        attributeFilter: ['src', 'type', 'label', 'data-setup', 'class', 'style', 'hidden', 'data-ad', 'data-is-ad', 'data-ad-playing'] });
      this.observers.set(root, observer);
    }
    for (const [root, observer] of this.observers) {
      if (root instanceof ShadowRoot && !root.host.isConnected) { observer.disconnect(); this.observers.delete(root); }
    }
    for (const video of data.snapshot.videos || []) {
      const evidence = video.currentSrc && this.network.get(video.currentSrc);
      if (evidence) {
        video.sources = [...(video.sources || []), { src: evidence.url,
          type: evidence.protocol === 'hls' ? 'application/vnd.apple.mpegurl' : evidence.protocol === 'dash' ? 'application/dash+xml' : 'video/mp4' }];
      }
    }
    const present = new Set<string>();
    for (const group of collectPlayerGroups(data.snapshot)) {
      const target = data.targets.get(group.playerId)!;
      const primary = data.videos.get(group.playerId)?.[0];
      if (!data.adPlaying.has(group.playerId)) {
        if (this.playerVideos.has(target) && this.playerVideos.get(target) !== primary) {
          this.identities.set(target, 'player-' + ++this.nextId);
        }
        this.playerVideos.set(target, primary);
      }
      let instance = this.identities.get(target);
      if (!instance) { instance = 'player-' + ++this.nextId; this.identities.set(target, instance); }
      const id = 'player:' + this.epoch + ':' + instance;
      present.add(id);
      if (!this.observed.has(target)) {
        this.resize?.observe(target); this.visibility?.observe(target); this.observed.add(target);
      }
      if (this.targets.has(id) && this.targets.get(id) !== target) {
        this.detected.delete(id); this.signatures.delete(id); this.opts.onRemoved?.(id);
      }
      this.targets.set(id, target);
      const previous = this.detected.get(id);
      const visible = playerVisible(target) && this.intersections.get(target) !== false;
      this.opts.onVisibility?.(id, visible);
      if (previous?.discovery_status === 'deferred' && visible) this.signatures.delete(id);
      // Ad playback on a reused video element must not replace its known main source.
      if (data.adPlaying.has(group.playerId) && !group.sources.length && previous && previous.protocol !== 'webpage') continue;
      const signature = JSON.stringify([this.page, group.sources]);
      if (this.signatures.get(id) === signature) continue;
      if (!visible) continue;
      const sources = group.sources;
      // Page-level extraction cannot safely choose among unrelated unresolved players.
      const extraction = !sources.length && data.targets.size === 1 && publicMediaUrl(location.href);
      if (!sources.length && !extraction) {
        if (previous) {
          this.detected.delete(id); this.signatures.delete(id); this.opts.onRemoved?.(id);
        }
        continue;
      }
      if (extraction && document.visibilityState === 'hidden') continue;
      // IntersectionObserver includes ancestor-frame clipping, which local
      // viewport geometry alone cannot determine for a cross-origin iframe.
      if (extraction && this.visibility && this.intersections.get(target) !== true) continue;
      const first = sources[0];
      const media: DetectedMedia = {
        id, page_url: this.page, page_title: document.title, referrer: this.page,
        master_url: first?.url || this.page, protocol: first?.protocol || 'webpage',
        ...(extraction ? { extraction_url: this.page, discovery_status: 'detecting' as const } : {}),
        variants: sources.map((source, index) => ({ url: source.url,
          label: source.label || (sources.length === 1 ? 'Original' : 'Source ' + (index + 1)),
          protocol: source.protocol, fps: source.fps, width: source.width, height: source.height })),
        element_rect: target.getBoundingClientRect(),
      };
      this.signatures.set(id, signature); this.detected.set(id, media);
      this.opts.onDetected(media, target);
    }
    for (const id of this.detected.keys()) {
      if (!present.has(id)) {
        const target = this.targets.get(id);
        if (target) { this.resize?.unobserve(target); this.visibility?.unobserve(target); }
        this.detected.delete(id); this.signatures.delete(id); this.targets.delete(id); this.opts.onRemoved?.(id);
      }
    }
  }

  getMediaNearPoint(x: number, y: number): DetectedMedia | null {
    const candidates = [...this.targets].filter(([id, target]) => {
      const rect = target.getBoundingClientRect();
      return this.isVisible(id)
        && x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    });
    for (const element of document.elementsFromPoint?.(x, y) || []) {
      const matches = candidates.filter(([, target]) => target === element || target.contains(element)
        || element.shadowRoot?.contains(target));
      if (matches.length === 1) return this.detected.get(matches[0][0]) || null;
      if (matches.length > 1) return null;
    }
    return candidates.length === 1 ? this.detected.get(candidates[0][0]) || null : null;
  }
  getAnyDetectedMedia(): DetectedMedia | null {
    return this.targets.size === 1 && this.detected.size === 1 ? this.detected.values().next().value || null : null;
  }
}
