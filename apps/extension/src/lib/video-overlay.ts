/**
 * Video Overlay — IDM-style download button on detected videos.
 *
 * Design: A single compact pill button at the top-right of the video.
 * Click → downloads best quality (single variant) OR opens quality picker (multi).
 *
 * ISOLATION: Each overlay lives inside a closed Shadow DOM so host-page
 * CSS cannot interfere (Pornhub, Aparat, etc.).
 *
 * Uses `position: fixed` on `<html>`, tracks video via getBoundingClientRect().
 */

import type { DetectedMedia, MediaVariant, MediaDownloadRequest } from './media-types';
import { playerVisible } from './player-dom';

const OVERLAY_ATTR = 'data-dlman-overlay-id';
const Z = 2147483647;
const dismissed = new Set<string>();

// ============================================================================
// Styles (inside Shadow DOM — fully isolated from host page)
// ============================================================================

function shadowCSS(): string {
  return `
:host{all:initial;position:fixed;z-index:${Z};display:block;pointer-events:none;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif}
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}

/* Main pill button */
.pill{display:inline-flex;align-items:center;gap:6px;height:32px;padding:0 12px 0 8px;background:rgba(22,22,22,.92);color:#e8e8e8;font-size:12px;font-weight:600;border:1px solid rgba(255,255,255,.12);border-radius:7px;cursor:pointer;pointer-events:auto;white-space:nowrap;backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px);box-shadow:0 2px 12px rgba(0,0,0,.5);opacity:0;transform:translateY(-4px) scale(.96);transition:opacity .25s ease,transform .25s ease,background .15s,border-color .15s;position:relative;user-select:none}
.pill.show{opacity:1;transform:translateY(0) scale(1)}
.pill:hover{background:rgba(30,30,30,.96);border-color:rgba(59,130,246,.5)}
.pill.hide{display:none}

/* Favicon / logo */
.logo{width:18px;height:18px;flex-shrink:0;display:flex;align-items:center;justify-content:center}
.logo img{width:18px;height:18px;object-fit:contain}

/* Chevron for multi-quality */
.chev{width:8px;height:8px;color:#888;margin-left:2px;transition:color .15s,transform .2s}
.pill:hover .chev{color:#bbb}
.pill.open .chev{transform:rotate(180deg)}

/* Close X */
.x{position:absolute;top:-6px;right:-6px;width:16px;height:16px;border-radius:50%;background:#333;color:#999;border:1px solid #555;font-size:10px;line-height:14px;text-align:center;cursor:pointer;opacity:0;transition:opacity .15s,background .12s,color .12s;pointer-events:auto}
.pill:hover .x{opacity:1}
.x:hover{background:#c0392b;color:#fff;border-color:#c0392b}

/* Dropdown */
.dd{position:absolute;top:calc(100% + 6px);right:0;min-width:220px;background:rgba(22,22,22,.96);border:1px solid rgba(255,255,255,.12);border-radius:8px;box-shadow:0 8px 32px rgba(0,0,0,.7);backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);padding:4px 0;opacity:0;transform:translateY(-4px);transition:opacity .15s,transform .15s;pointer-events:none}
.dd.open{opacity:1;transform:translateY(0);pointer-events:auto}
.dd-hdr{padding:6px 12px 4px;font-size:9px;font-weight:700;color:#666;text-transform:uppercase;letter-spacing:.08em}
.dd-item{display:flex;align-items:center;justify-content:space-between;gap:8px;padding:7px 12px;font-size:12px;color:#ddd;cursor:pointer;transition:background .1s;border-radius:0}
.dd-item:hover{background:rgba(59,130,246,.35);color:#fff}
.dd-lbl{font-weight:600}
.dd-meta{font-size:9.5px;color:#777}
.dd-item:hover .dd-meta{color:rgba(255,255,255,.6)}
.dd-item-all{border-bottom:none}
.dd-item-all .dd-lbl{color:#60a5fa}
.dd-item-all:hover{background:rgba(59,130,246,.45)}
`;
}

// ============================================================================
// SVG
// ============================================================================

function chevSvg(): string {
  return `<svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>`;
}

// ============================================================================
// Helpers
// ============================================================================

function fmtSize(b: number): string {
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${(b / 1024).toFixed(0)} KB`;
  if (b < 1073741824) return `${(b / 1048576).toFixed(1)} MB`;
  return `${(b / 1073741824).toFixed(2)} GB`;
}

function fmtBitrate(bps: number): string {
  if (bps < 1000) return `${bps} bps`;
  if (bps < 1e6) return `${(bps / 1000).toFixed(0)} kbps`;
  return `${(bps / 1e6).toFixed(1)} Mbps`;
}

// ============================================================================
// VideoOverlayManager
// ============================================================================

export type OnDownloadRequest = (request: MediaDownloadRequest) => void | Promise<void>;

interface Overlay {
  media: DetectedMedia;
  video: HTMLElement | null;
  host: HTMLElement;
  shadow: ShadowRoot;
  pill: HTMLElement;
  label: HTMLElement;
  dropdown: HTMLElement | null;
  rafId: number | null;
  outsideListener: ((e: MouseEvent) => void) | null;
}

export class VideoOverlayManager {
  private overlays = new Map<string, Overlay>();
  private onDownload: OnDownloadRequest;
  private scrollHandler: (() => void) | null = null;
  private resizeHandler: (() => void) | null = null;

  constructor(onDownload: OnDownloadRequest, private resolveMedia?: (media: DetectedMedia) => Promise<DetectedMedia>) {
    this.onDownload = onDownload;
    this.scrollHandler = () => this.repositionAll();
    this.resizeHandler = () => this.repositionAll();
    window.addEventListener('scroll', this.scrollHandler, { passive: true, capture: true });
    window.addEventListener('resize', this.resizeHandler, { passive: true });
  }

  // ---------- Public API ----------

  addOverlay(media: DetectedMedia, variants?: MediaVariant[], targetVideo?: HTMLElement): void {
    if (dismissed.has(media.id)) return;
    if (this.overlays.has(media.id)) {
      const o = this.overlays.get(media.id)!;
      o.media = variants ? { ...media, variants } : media;
      this.closeDropdown(o);
      this.renderState(o);
      return;
    }

    const video = targetVideo || this.findVideo(media);
    if (!video) return;

    const hasMulti = variants && variants.length > 1;

    // Shadow DOM host
    const host = document.createElement('dlman-overlay');
    host.style.cssText = `all:initial;position:fixed;z-index:${Z};display:block;pointer-events:none`;
    const shadow = host.attachShadow({ mode: 'closed' });

    const style = document.createElement('style');
    style.textContent = shadowCSS();
    shadow.appendChild(style);

    // Build pill: [Logo] Download Video [▾]  (×)
    const pill = document.createElement('div');
    pill.className = 'pill';
    pill.setAttribute('role', 'button');
    pill.tabIndex = 0;
    pill.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); pill.click(); }
    });

    // Logo badge
    const logo = document.createElement('div');
    logo.className = 'logo';
    const logoImage = document.createElement('img');
    logoImage.src = new URL('icon/48.png', browser.runtime.getURL('/')).href;
    logoImage.alt = 'DLMan';
    logo.appendChild(logoImage);
    pill.appendChild(logo);

    // Label
    const label = document.createElement('span');
    label.textContent = hasMulti ? 'Download Video' : 'Download Video';
    pill.appendChild(label);

    // Chevron (multi-quality indicator)
    if (hasMulti) {
      const chev = document.createElement('span');
      chev.dataset.qualityChevron = 'true';
      chev.innerHTML = chevSvg();
      pill.appendChild(chev);
    }

    // Main click: single → download immediately, multi → toggle quality picker
    pill.addEventListener('click', async (e) => {
      e.stopPropagation();
      e.preventDefault();
      const current = this.overlays.get(media.id);
      if (!current || pill.dataset.loading || ['detecting', 'deferred'].includes(current.media.discovery_status || '')) return;
      if (current.dropdown) { this.closeDropdown(current); return; }
      try {
        if (['youtube', 'webpage'].includes(current.media.protocol) && this.resolveMedia) {
          pill.dataset.loading = 'true'; label.textContent = 'Loading…';
          const resolved = await this.resolveMedia(current.media);
          if (this.overlays.get(media.id) !== current || !current.video?.isConnected) return;
          current.media = resolved;
        }
        label.textContent = 'Download Video'; pill.title = '';
        if (current.media.variants.length > 1) this.toggleDropdown(media.id, pill, current.media.variants);
        else if (current.media.variants.length === 1 || current.media.protocol !== 'youtube') await this.doDownload(current.media, current.media.variants, 0);
      } catch (error) {
        label.textContent = 'Retry';
        pill.title = error instanceof Error ? error.message : 'Video inspection failed.';
      } finally { delete pill.dataset.loading; }
    });

    // Close X (visible on hover)
    const x = document.createElement('div');
    x.className = 'x';
    x.textContent = '×';
    x.addEventListener('click', (e) => {
      e.stopPropagation();
      e.preventDefault();
      dismissed.add(media.id);
      this.removeOverlay(media.id);
    });
    pill.appendChild(x);

    shadow.appendChild(pill);

    const overlay: Overlay = {
      media: variants ? { ...media, variants } : media,
      video,
      host,
      shadow,
      pill,
      label,
      dropdown: null,
      rafId: null,
      outsideListener: null,
    };

    this.positionFixed(host, video);
    document.documentElement.appendChild(host);
    requestAnimationFrame(() => pill.classList.add('show'));

    this.overlays.set(media.id, overlay);
    this.renderState(overlay);
    video.setAttribute(OVERLAY_ATTR, media.id);
  }

  private renderState(overlay: Overlay): void {
    const detecting = ['detecting', 'deferred'].includes(overlay.media.discovery_status || '');
    overlay.pill.setAttribute('aria-disabled', String(detecting));
    overlay.pill.style.cursor = detecting ? 'wait' : 'pointer';
    overlay.label.textContent = detecting ? 'Detecting video…'
      : overlay.media.discovery_status === 'failed' ? 'Retry' : 'Download Video';
    overlay.pill.title = overlay.media.discovery_error || '';
    let chevron = overlay.pill.querySelector<HTMLElement>('[data-quality-chevron]');
    if (overlay.media.variants.length > 1 && !chevron) {
      chevron = document.createElement('span');
      chevron.dataset.qualityChevron = 'true'; chevron.innerHTML = chevSvg();
      overlay.pill.appendChild(chevron);
    }
    if (chevron) chevron.style.display = overlay.media.variants.length > 1 ? '' : 'none';
  }

  removeOverlay(id: string): void {
    const o = this.overlays.get(id);
    if (o) {
      this.cleanup(o);
      this.overlays.delete(id);
    }
  }

  setVisibility(id: string, visible: boolean): void {
    const overlay = this.overlays.get(id);
    if (!overlay?.video) return;
    if (visible) this.positionFixed(overlay.host, overlay.video);
    else { overlay.host.style.display = 'none'; this.closeDropdown(overlay); }
  }

  destroy(): void {
    for (const o of this.overlays.values()) this.cleanup(o);
    this.overlays.clear();
    if (this.scrollHandler) window.removeEventListener('scroll', this.scrollHandler, true);
    if (this.resizeHandler) window.removeEventListener('resize', this.resizeHandler);
  }

  // ---------- Positioning ----------

  private repositionAll(): void {
    for (const o of this.overlays.values()) {
      if (!o.video) continue;
      if (o.rafId) cancelAnimationFrame(o.rafId);
      o.rafId = requestAnimationFrame(() => {
        this.positionFixed(o.host, o.video!);
        o.rafId = null;
      });
    }
  }

  private positionFixed(host: HTMLElement, video: HTMLElement): void {
    if (!playerVisible(video)) { host.style.display = 'none'; return; }
    const vr = video.getBoundingClientRect();
    if (!video.isConnected || vr.width <= 0 || vr.height <= 0 || vr.bottom < 0 || vr.top > window.innerHeight || vr.right < 0 || vr.left > window.innerWidth) {
      host.style.display = 'none';
      return;
    }
    host.style.display = 'block';
    const top = vr.top + 8;
    const right = window.innerWidth - vr.right + 8;
    host.style.top = `${top}px`;
    host.style.right = `${right}px`;
    host.style.left = 'auto';
  }

  // ---------- Dropdown ----------

  private toggleDropdown(id: string, anchor: HTMLElement, variants: MediaVariant[]): void {
    const o = this.overlays.get(id);
    if (!o) return;
    if (o.dropdown) { this.closeDropdown(o); return; }

    const dd = document.createElement('div');
    dd.className = 'dd';

    const hdr = document.createElement('div');
    hdr.className = 'dd-hdr';
    hdr.textContent = 'Select Quality';
    dd.appendChild(hdr);

    // "Download All" option — downloads every variant at once
    const videoVariants = variants.filter((variant) => !variant.audio_only);
    if ((o.media.protocol === "youtube" ? videoVariants.length : variants.length) > 1) {
      const allItem = document.createElement('div');
      allItem.className = 'dd-item dd-item-all';
      const allLbl = document.createElement('span');
      allLbl.className = 'dd-lbl';
      allLbl.textContent = '⬇ Download All';
      const allMeta = document.createElement('span');
      allMeta.className = 'dd-meta';
      allMeta.textContent = `${o.media.protocol === "youtube" ? videoVariants.length : variants.length} qualities`;
      allItem.appendChild(allLbl);
      allItem.appendChild(allMeta);
      allItem.addEventListener('click', async (e) => {
        e.stopPropagation();
        e.preventDefault();
        try {
          await this.doDownloadAll(o.media, variants);
          this.closeDropdown(o);
        } catch (error) {
          o.pill.title = error instanceof Error ? error.message : 'Download was not accepted.';
          allLbl.textContent = 'Retry';
        }
      });
      dd.appendChild(allItem);

      // Separator
      const sep = document.createElement('div');
      sep.style.cssText = 'height:1px;background:rgba(255,255,255,.08);margin:4px 8px';
      dd.appendChild(sep);
    }

    variants.forEach((v, i) => {
      const item = document.createElement('div');
      item.className = 'dd-item';

      const lbl = document.createElement('span');
      lbl.className = 'dd-lbl';
      lbl.textContent = v.label;

      const meta = document.createElement('span');
      meta.className = 'dd-meta';
      const parts: string[] = [];
      if (v.codecs) parts.push(v.codecs.split(',')[0]);
      if (v.estimated_size) parts.push(fmtSize(v.estimated_size));
      if (v.bandwidth) parts.push(fmtBitrate(v.bandwidth));
      if (v.fps) parts.push(v.fps + ' fps');
      meta.textContent = parts.join(' · ');

      item.appendChild(lbl);
      if (parts.length) item.appendChild(meta);
      item.addEventListener('click', async (e) => {
        e.stopPropagation();
        e.preventDefault();
        try {
          await this.doDownload(o.media, variants, i);
          this.closeDropdown(o);
        } catch (error) {
          o.pill.title = error instanceof Error ? error.message : 'Download was not accepted.';
          lbl.textContent = 'Retry';
        }
      });
      dd.appendChild(item);
    });

    anchor.appendChild(dd);
    o.dropdown = dd;
    anchor.classList.add('open');
    requestAnimationFrame(() => dd.classList.add('open'));

    const outside = (e: MouseEvent) => {
      const path = e.composedPath();
      if (!path.includes(o.host)) {
        this.closeDropdown(o);
      }
    };
    o.outsideListener = outside;
    setTimeout(() => document.addEventListener('click', outside, true), 0);
  }

  private closeDropdown(o: Overlay): void {
    if (!o.dropdown) return;
    o.pill.classList.remove('open');
    o.dropdown.classList.remove('open');
    if (o.outsideListener) {
      document.removeEventListener('click', o.outsideListener, true);
      o.outsideListener = null;
    }
    const dd = o.dropdown;
    o.dropdown = null;
    setTimeout(() => dd.remove(), 180);
  }

  // ---------- Download ----------

  private async doDownload(media: DetectedMedia, variants?: MediaVariant[], idx?: number): Promise<void> {
    const selected = variants?.[idx ?? 0];
    const selectedMedia = !['youtube', 'webpage'].includes(media.protocol) && selected
      ? { ...media, master_url: selected.master_url || selected.url, protocol: selected.protocol || media.protocol } : media;
    await this.onDownload({ media: variants ? { ...selectedMedia, variants } : selectedMedia, variant_index: idx,
      inspection_id: media.inspection_id, youtube_selection: selected?.youtube_selection,
      web_media_selection: selected?.web_media_selection,
      ...(selected?.master_url ? { media_quality: { label: selected.label, height: selected.height,
        fps: selected.fps ? Math.round(selected.fps) : undefined } } : {}) });
  }

  /** Download every variant — fires a separate request for each quality */
  private async doDownloadAll(media: DetectedMedia, variants: MediaVariant[]): Promise<void> {
    for (let i = 0; i < variants.length; i++) {
      if (media.protocol === 'youtube' && variants[i].audio_only) continue;
      await this.doDownload(media, variants, i);
    }
  }

  // ---------- Find Video ----------

  private findVideo(media: DetectedMedia): HTMLVideoElement | null {
    for (const v of document.querySelectorAll('video')) {
      const el = v as HTMLVideoElement;
      if (el.getAttribute(OVERLAY_ATTR)) continue;
      const src = el.currentSrc || el.src;
      if (src && this.urlMatch(src, media.master_url)) return el;
      for (const s of el.querySelectorAll('source')) {
        if ((s as HTMLSourceElement).src && this.urlMatch((s as HTMLSourceElement).src, media.master_url)) return el;
      }
    }
    return null;
  }

  private urlMatch(a: string, b: string): boolean {
    try {
      const ua = new URL(a);
      const ub = new URL(b);
      return ua.href === ub.href;
    } catch { return a === b; }
  }

  // ---------- Cleanup ----------

  private cleanup(o: Overlay): void {
    o.host.remove();
    if (o.rafId) cancelAnimationFrame(o.rafId);
    if (o.video) o.video.removeAttribute(OVERLAY_ATTR);
    if (o.outsideListener) document.removeEventListener('click', o.outsideListener, true);
  }
}
