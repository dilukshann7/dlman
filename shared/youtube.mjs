const HOSTS = new Set(['youtube.com', 'www.youtube.com', 'm.youtube.com', 'youtu.be', 'www.youtu.be']);

export function isYoutubeHost(value) {
  try { return HOSTS.has(new URL(value).hostname); } catch { return false; }
}

export function canonicalYoutubeUrl(value) {
  try {
    if (value.length > 8192) return null;
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || !HOSTS.has(url.hostname)
      || url.username || url.password || url.port) return null;
    const short = url.hostname === 'youtu.be' || url.hostname === 'www.youtu.be';
    const id = short ? url.pathname.slice(1)
      : url.pathname === '/watch' ? url.searchParams.get('v')
      : /^\/shorts\/([^/]+)\/?$/.exec(url.pathname)?.[1];
    return /^[A-Za-z0-9_-]{11}$/.test(id || '') ? `https://www.youtube.com/watch?v=${id}` : null;
  } catch { return null; }
}

export function videoChoices(variants) {
  return variants.filter((variant) => variant.output_mode !== 'mp3');
}

/** Shared short-request polling; stale navigation can never return a usable preview. */
export async function awaitYoutubeInspection(start, poll, options = {}) {
  const now = options.now || Date.now;
  const current = options.isCurrent || (() => true);
  const delay = options.delay || (() => new Promise((resolve) => setTimeout(resolve, 1000)));
  const deadline = now() + (options.timeout ?? 175_000);
  let result = await start();
  while (result?.status === 'pending') {
    if (!current()) throw new Error('Video changed.');
    if (now() > deadline) throw new Error('Video inspection timed out. Retry.');
    await delay();
    if (!current()) throw new Error('Video changed.');
    result = await poll(result.id);
  }
  if (!current()) throw new Error('Video changed.');
  if (result?.status !== 'ready' || !result.preview) {
    throw new Error(result?.error || 'DLMan is unavailable. Open the desktop app and retry.');
  }
  return result;
}

/** Keep Download All handoffs reviewable through the existing single dialog. */
export function createYoutubeDialogQueue(open, isOpen, schedule = (callback) => setTimeout(callback, 0)) {
  const pending = [];
  let scheduled = false;
  let destroyed = false;
  return {
    offer(payload) {
      if (destroyed) return;
      if (scheduled || isOpen()) pending.push(payload);
      else open(payload);
    },
    closed() {
      if (destroyed || scheduled || !pending.length) return;
      scheduled = true;
      schedule(() => {
        scheduled = false;
        if (!destroyed && !isOpen() && pending.length) open(pending.shift());
      });
    },
    destroy() { destroyed = true; pending.length = 0; },
  };
}
