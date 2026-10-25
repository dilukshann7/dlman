const AD_HOSTS = [
  'doubleclick.net', 'googlesyndication.com', 'googleadservices.com', 'moatads.com',
  'imasdk.googleapis.com', 'serving-sys.com', 'sabavision.com',
];
const AD_LABEL = /^(?:adserver|adsystem|analytics)(?:[-\d].*)?$/i;
const AD_PATH = /(?:^|\/)(?:ads?|adverts?|advertising|preroll|midroll|postroll|vast|vpaid|tracking)(?:[._\/-]|$)/i;
const NOISE_PATH = /(?:^|\/)(?:segments?|chunks?|init)(?:[._\/-]|$)|\.(?:ts|m4s|cmfv|cmfa|aac|mp3|m4a|vtt|srt|key)$/i;

/** Strict resource URL validation; credentials and private browser/desktop URLs never leave the page. */
export function publicMediaUrl(value) {
  try {
    if (typeof value !== 'string' || value.length > 16384 || /[\x00-\x1f\x7f]/.test(value)) return null;
    const parsed = new URL(value);
    const host = parsed.hostname.toLowerCase();
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password
      || host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local')
      || host.includes(':') || /^\d+(?:\.\d+){3}$/.test(host) || !host.includes('.')) return null;
    return parsed;
  } catch { return null; }
}

export function isAdResource(value) {
  const parsed = publicMediaUrl(value);
  if (!parsed) return false;
  let path = parsed.pathname;
  try { path = decodeURIComponent(path); } catch { /* Keep the original path when escaping is malformed. */ }
  return AD_HOSTS.some((host) => parsed.hostname === host || parsed.hostname.endsWith('.' + host))
    || parsed.hostname.split('.').some((label) => AD_LABEL.test(label))
    || AD_PATH.test(path)
    || (parsed.hostname === 'facebook.com' && /^\/tr(?:\/|$)/i.test(path));
}

/** Classify complete video resources, never ads or individual adaptive transport pieces. */
export function classifyMediaCandidate(candidate) {
  if (!candidate || candidate.isAd) return null;
  const parsed = publicMediaUrl(candidate.url);
  if (!parsed || isAdResource(parsed.href)) return null;
  let path = parsed.pathname.replace(/\/+$/, '');
  try { path = decodeURIComponent(path); } catch { /* See isAdResource. */ }
  const mime = typeof candidate.mime === 'string' ? candidate.mime.toLowerCase().split(';')[0].trim() : '';
  const hls = ['application/vnd.apple.mpegurl', 'application/x-mpegurl', 'audio/mpegurl', 'audio/x-mpegurl'].includes(mime);
  if (/\.(?:ts|m4s|cmfv|cmfa|aac|mp3|m4a|vtt|srt|key)$/i.test(path)
    || mime === 'video/mp2t' || mime === 'application/octet-stream' && /\.(?:bin|dat)$/i.test(path)
    || mime.startsWith('audio/') && !hls) return null;
  let protocol;
  if (/\.m3u8$/i.test(path) || hls) protocol = 'hls';
  else if (/\.mpd$/i.test(path) || mime === 'application/dash+xml') protocol = 'dash';
  else if (NOISE_PATH.test(path)) return null;
  else if (/\.(?:mp4|webm|mkv|mov|m4v|flv|ogv)$/i.test(path)
    || ['video/mp4', 'video/webm', 'video/ogg', 'video/quicktime', 'video/x-matroska'].includes(mime)) protocol = 'direct';
  else return null;
  parsed.hash = '';
  return { url: parsed.href, protocol };
}
