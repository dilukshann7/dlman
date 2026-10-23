export function canRefreshDownload(download) {
  if (!download || !['paused', 'failed'].includes(download.status) || download.youtube != null) return false;
  try {
    const url = new URL(download.url);
    const host = url.hostname.toLowerCase();
    return ['http:', 'https:'].includes(url.protocol)
      && ![ 'youtube.com', 'youtu.be', 'youtube-nocookie.com', 'googlevideo.com' ].some(domain => host === domain || host.endsWith(`.${domain}`))
      && !/\.(m3u8|mpd)(\/|$)/i.test(url.pathname);
  } catch { return false; }
}

export function refreshSessionExpired(session, now = Date.now()) {
  return !session || now >= session.expires_at;
}

export function displayRefreshUrl(value) {
  try {
    const url = new URL(value);
    return `${url.origin}${url.pathname}`;
  } catch { return ''; }
}
