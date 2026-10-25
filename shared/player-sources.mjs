import { captureSource } from './web-media-capture.mjs';

const MAX_SCRIPT_CHARACTERS = 2 * 1024 * 1024;
// Strings and comments are single tokens. Player configuration is read as literal
// data only: expressions, template literals and functions are never evaluated.
const JS_TOKEN = /\/\/[^\r\n]*|\/\*[\s\S]*?\*\/|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|`(?:\\[\s\S]|[^`\\])*`|[A-Za-z_$][\w$]*|[^\s]/g;

function stringLiteral(token) {
  if (!token || !['"', "'"].includes(token[0]) || token.at(-1) !== token[0]) return null;
  return token.slice(1, -1).replace(/\\(?:u([\da-f]{4})|x([\da-f]{2})|(\r\n|[\r\n])|([\s\S]))/gi,
    (_, unicode, hex, continuation, character) => unicode || hex ? String.fromCharCode(parseInt(unicode || hex, 16))
      : continuation ? '' : ({ n: '\n', r: '\r', t: '\t', b: '\b', f: '\f', v: '\v', 0: '\0' }[character] ?? character));
}

function flashvarConfigs(script) {
  const configs = [];
  let state = 0, depth = 0, property = [], fields = Object.create(null);
  const finishProperty = () => {
    if (property.length === 3 && property[1] === ':') {
      const key = stringLiteral(property[0]) ?? property[0];
      const value = stringLiteral(property[2]);
      if (/^(?:video_title|video_(?:url|alt_url\d*)(?:_text)?)$/.test(key) && value !== null) fields[key] = value;
    }
    property = [];
  };
  for (const match of script.matchAll(JS_TOKEN)) {
    const token = match[0];
    if (token.startsWith('//') || token.startsWith('/*')) continue;
    if (!depth) {
      if (state === 2 && token === '{') { depth = 1; property = []; fields = Object.create(null); state = 0; }
      else if (state === 1 && token === '=') state = 2;
      else state = token === 'flashvars' ? 1 : 0;
      continue;
    }
    if (depth === 1 && (token === ',' || token === '}')) {
      finishProperty();
      if (token === '}') { configs.push(fields); depth = 0; if (configs.length >= 32) break; }
      continue;
    }
    // Retain at most four tokens per property. Anything except one string literal
    // after its key and colon is intentionally ignored, including concatenation.
    if (depth === 1 && property.length < 4) property.push(token);
    if (['{', '[', '('].includes(token)) depth++;
    else if (['}', ']', ')'].includes(token)) depth--;
  }
  return configs;
}

/** Snapshot-based discovery works before preload and across browser DOM realms. */
function collectBasicSources({ baseUrl, title = '', videos = [], scripts = [] }) {
  const found = new Map();
  const add = (rawUrl, mime, label, sourceTitle = title, metadata = {}) => {
    if (typeof rawUrl !== 'string' || !rawUrl || rawUrl.length > 16384 || /[\x00-\x1f\x7f]/.test(rawUrl)
      || rawUrl.startsWith('function/')) return;
    let url;
    try { url = new URL(rawUrl, baseUrl).href; } catch { return; }
    const source = captureSource({ url, mime, title: label ? `${sourceTitle} · ${label}` : sourceTitle });
    if (source && (found.has(source.url) || found.size < 32)) {
      for (const key of ['fps', 'width', 'height']) {
        const value = Number(metadata[key]);
        if (Number.isFinite(value) && value > 0) source[key] = value;
      }
      found.set(source.url, { ...source, ...(label ? { label: String(label) } : {}) });
    }
  };
  for (const video of videos.slice(0, 32)) {
    add(video.currentSrc || video.src, video.type, video.label, title, video);
    if (video.src && video.src !== video.currentSrc) add(video.src, video.type, video.label, title, video);
    for (const source of (video.sources || []).slice(0, 32)) add(source.src, source.type, source.label, title, source);
  }
  let remaining = MAX_SCRIPT_CHARACTERS;
  for (const script of scripts) {
    if (typeof script !== 'string' || script.length > remaining) continue;
    remaining -= script.length;
    if (!script.includes('flashvars')) continue;
    for (const config of flashvarConfigs(script)) {
      for (const key of Object.keys(config).filter((key) => /^video_(?:url|alt_url\d*)$/.test(key))) {
        add(config[key], undefined, config[`${key}_text`], config.video_title || title);
      }
    }
  }
  return [...found.values()];
}

// This parser accepts literal data only. A property containing an expression is
// discarded in full, rather than accepting a potentially different URL prefix.
const DATA_TOKEN = /\/\/[^\r\n]*|\/\*[\s\S]*?\*\/|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|[A-Za-z_$][\w$]*|-?\d+(?:\.\d+)?|[^\s]/g;
function dataTokens(text) {
  return [...text.matchAll(DATA_TOKEN)].map((m) => m[0]).filter((t) => !t.startsWith('//') && !t.startsWith('/*'));
}
function literal(tokens, start, depth = 0) {
  if (depth > 16) return { end: start + 1 };
  const token = tokens[start];
  const string = stringLiteral(token);
  if (string !== null) return { value: string, end: start + 1 };
  if (/^-?\d+(?:\.\d+)?$/.test(token || '')) return { value: Number(token), end: start + 1 };
  if (['true', 'false', 'null'].includes(token)) return { value: JSON.parse(token), end: start + 1 };
  if (!['{', '['].includes(token)) return { end: start + 1 };
  const array = token === '[', closing = array ? ']' : '}';
  const result = array ? [] : Object.create(null);
  let i = start + 1, count = 0;
  while (i < tokens.length && tokens[i] !== closing && count++ < 256) {
    let key;
    if (!array) {
      key = stringLiteral(tokens[i]) ?? tokens[i];
      if (tokens[i + 1] !== ':') return { end: i + 1 };
      i += 2;
    }
    const parsed = literal(tokens, i, depth + 1);
    i = parsed.end;
    let valid = parsed.value !== undefined;
    if (![',', closing].includes(tokens[i])) {
      valid = false;
      let nested = 0;
      while (i < tokens.length) {
        const t = tokens[i];
        if (!nested && [',', closing].includes(t)) break;
        if (['(', '[', '{'].includes(t)) nested++;
        if ([')', ']', '}'].includes(t)) nested--;
        i++;
      }
    }
    if (valid) {
      if (array) result.push(parsed.value);
      else if (!['__proto__', 'constructor', 'prototype'].includes(key)) result[key] = parsed.value;
    }
    if (tokens[i] === ',') i++;
  }
  return tokens[i] === closing ? { value: result, end: i + 1 } : { end: i };
}

function playerConfigs(text) {
  if (!text.includes('jwplayer') && !text.includes('videojs')) return [];
  const tokens = dataTokens(text), configs = [];
  for (let i = 0; i < tokens.length && configs.length < 32; i++) {
    if (!['jwplayer', 'videojs'].includes(tokens[i]) || tokens[i + 1] !== '(') continue;
    const id = stringLiteral(tokens[i + 2]);
    if (!id) continue;
    let start = i + 4;
    if (tokens[i + 3] === ',' && tokens[i] === 'videojs') start = i + 4;
    else {
      if (tokens[i + 3] !== ')' || tokens[i + 4] !== '.' || !['setup', 'src'].includes(tokens[i + 5])
        || tokens[i + 6] !== '(') continue;
      start = i + 7;
    }
    const parsed = literal(tokens, start);
    if (parsed.value !== undefined && [')', ','].includes(tokens[parsed.end])) configs.push({ id, value: parsed.value });
  }
  return configs;
}

function sourceData(value, add, depth = 0) {
  if (depth > 12 || !value || typeof value !== 'object') return;
  if (Array.isArray(value)) { value.slice(0, 32).forEach((entry) => sourceData(entry, add, depth + 1)); return; }
  if (value.isAd === true || value.is_ad === true || value.ad === true) return;
  if (typeof value.src === 'string' || typeof value.file === 'string') {
    add({ src: value.src || value.file, type: value.type, label: value.label || value.res,
      fps: value.fps || value.framerate, width: value.width, height: value.height });
  }
  // Deliberately do not walk advertising, tracks, poster, preview or arbitrary fields.
  for (const key of ['sources', 'playlist']) if (value[key]) sourceData(value[key], add, depth + 1);
}

/** Keep per-player association explicit; page-wide metadata is safe only with a single player. */
export function collectPlayerGroups({ baseUrl, title = '', videos = [], scripts = [] }) {
  const groups = videos.slice(0, 32).map((video, index) => ({
    playerId: video.id || String(index), sources: collectBasicSources({ baseUrl, title, videos: [video] }),
  }));
  const add = (group, data) => {
    const source = collectBasicSources({ baseUrl, title, videos: [{ sources: [data] }] })[0];
    if (!source) return;
    for (const key of ['fps', 'width', 'height']) {
      const number = Number(data[key]);
      if (Number.isFinite(number) && number > 0) source[key] = number;
    }
    const existing = group.sources.findIndex((entry) => entry.url === source.url);
    if (existing >= 0) group.sources[existing] = { ...group.sources[existing], ...source };
    else if (group.sources.length < 32) group.sources.push(source);
  };
  videos.slice(0, 32).forEach((video, index) => {
    if (!video.setup || video.setup.length > 65536) return;
    try { sourceData(JSON.parse(video.setup), (data) => add(groups[index], data)); } catch { /* Invalid data-setup. */ }
  });
  let remaining = MAX_SCRIPT_CHARACTERS;
  for (const script of scripts) {
    const text = typeof script === 'string' ? script : script.text;
    if (typeof text !== 'string' || text.length > remaining) continue;
    remaining -= text.length;
    for (const config of playerConfigs(text)) {
      const matches = groups.filter((entry, index) => (videos[index].configId || entry.playerId) === config.id);
      if (matches.length === 1) sourceData(config.value, (data) => add(matches[0], data));
    }
    if (groups.length !== 1) continue;
    if (text.includes('flashvars')) {
      for (const source of collectBasicSources({ baseUrl, title, scripts: [text] })) {
        add(groups[0], { src: source.url, label: source.label });
      }
    }
    if (typeof script === 'object' && script.type === 'application/ld+json' && text.length <= 262144) {
      try {
        const visit = (value, depth = 0) => {
          if (depth > 12 || !value || typeof value !== 'object') return;
          if (Array.isArray(value)) { value.slice(0, 32).forEach((item) => visit(item, depth + 1)); return; }
          if ([value['@type']].flat().includes('VideoObject') && typeof value.contentUrl === 'string') {
            add(groups[0], { src: value.contentUrl, label: value.name, width: value.width, height: value.height });
          }
          if (value['@graph']) visit(value['@graph'], depth + 1);
        };
        visit(JSON.parse(text));
      } catch { /* Metadata is untrusted literal data. */ }
    }
  }
  return groups;
}

/** Capture retains a source list; overlays use collectPlayerGroups to avoid guessing ownership. */
export function collectPlayerSources(snapshot) {
  const groups = collectPlayerGroups({ ...snapshot, videos: snapshot.videos?.length ? snapshot.videos : [{ id: 'page' }] });
  const result = new Map();
  for (const group of groups) for (const source of group.sources) {
    if (result.has(source.url) || result.size < 32) result.set(source.url, source);
  }
  return [...result.values()];
}
