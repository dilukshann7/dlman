import { isYoutubeHost } from './youtube.mjs';

export function isWebMediaUrl(value) {
  try {
    const url = new URL(value);
    return value.length <= 8192 && ['http:', 'https:'].includes(url.protocol)
      && !url.username && !url.password && !isYoutubeHost(value);
  } catch { return false; }
}

/** Preserve every available source; prefer resolution, frame rate, then MP4. */
export function sortWebMediaVariants(variants) {
  const resolution = (variant) => Number(/(\d+)\s*p(?:\b|\d)/i.exec(variant.quality)?.[1]
    || /\d+\s*[x×]\s*(\d+)/i.exec(variant.quality)?.[1] || 0);
  return variants.filter((variant) => variant.output_mode === 'video').slice().sort((a, b) =>
    resolution(b) - resolution(a) || (b.fps || 0) - (a.fps || 0)
    || Number(b.container.toLowerCase() === 'mp4') - Number(a.container.toLowerCase() === 'mp4'));
}

export function webMediaFilename(title, container, edited) {
  const base = edited ? edited.replace(/\.[^.\\/]+$/, '') : title;
  const safe = base.replace(/[\/\\:*?"<>|\u0000-\u001f]/g, '_').trim().replace(/[. ]+$/, '').slice(0, 160) || 'Video';
  return `${safe}.${container}`;
}

export function webMediaSelection(preview, variant) {
  return { identity: preview.identity, format_id: variant.id, container: variant.container,
    quality: variant.quality, output_mode: 'video' };
}

/** Resolve a browser HLS quality against the parent's complete video+audio formats. */
export function webMediaSelectionForQuality(preview, hint) {
  const height = (quality) => Number(/(\d+)\s*p(?:\b|\d)/i.exec(quality)?.[1] || 0);
  const expected = hint.height || height(hint.label);
  const matches = preview.variants.filter((variant) => variant.output_mode === 'video'
    && (expected ? height(variant.quality) === expected : variant.quality === hint.label)
    && (!hint.fps || variant.fps === hint.fps));
  const mp4 = matches.filter((variant) => variant.container === 'mp4');
  const selected = mp4.length === 1 ? mp4[0] : matches.length === 1 ? matches[0] : undefined;
  if (!selected) throw new Error('Selected quality is unavailable or ambiguous. Retry the quality list in DLMan.');
  return webMediaSelection(preview, selected);
}

export function validateWebMediaSelection(preview, selection) {
  return preview.identity === selection.identity && preview.variants.some((variant) =>
    variant.id === selection.format_id && variant.container === selection.container
    && variant.quality === selection.quality && variant.output_mode === selection.output_mode);
}

/** File responses alone may continue through ordinary file probing and transfer. */
export async function awaitWebMediaInspection(start, poll, options = {}) {
  const now = options.now || Date.now;
  const current = options.isCurrent || (() => true);
  const delay = options.delay || (() => new Promise((resolve) => setTimeout(resolve, 1000)));
  const deadline = now() + (options.timeout ?? 210_000);
  let result = await start();
  while (result?.status === 'pending') {
    if (!current()) throw new Error('URL changed.');
    if (now() > deadline) throw new Error('Video inspection timed out. Retry.');
    await delay();
    if (!current()) throw new Error('URL changed.');
    result = await poll(result.id);
  }
  if (!current()) throw new Error('URL changed.');
  if (result?.status === 'file') return result;
  if (result?.status !== 'ready' || !result.preview) throw new Error(result?.error || 'Video inspection failed. Retry or detect in browser.');
  if (!sortWebMediaVariants(result.preview.variants).length) throw new Error('No downloadable video found. Try Detect in browser.');
  return result;
}

export function webMediaCaptureBridge(capture) {
  try {
    const url = new URL(capture.bridge_url);
    if (url.protocol === 'http:' && url.hostname === '127.0.0.1' && url.port
      && !url.username && !url.password && url.pathname === '/web-media-capture'
      && !url.search && url.hash === `#${capture.id}`) return url.href;
  } catch { /* Report the same actionable error for absent and malformed bridges. */ }
  throw new Error('Invalid browser capture bridge. Update DLMan and retry.');
}
