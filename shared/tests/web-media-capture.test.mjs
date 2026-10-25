import test from 'node:test';

test('positive HLS and DASH manifests survive segment directory heuristics', () => {
  for (const directory of ['segments', 'chunks', 'init']) {
    assert.equal(captureSource({ url: `https://cdn.example/${directory}/master.m3u8`, mime: 'application/vnd.apple.mpegurl' })?.protocol, 'hls');
    assert.equal(captureSource({ url: `https://cdn.example/${directory}/manifest`, mime: 'application/dash+xml' })?.protocol, 'dash');
    assert.equal(captureSource({ url: `https://cdn.example/${directory}/segment-1.m4s`, mime: 'video/mp4' }), null);
  }
  assert.equal(captureSource({ url: 'https://cdn.example/ads/master.m3u8', mime: 'application/vnd.apple.mpegurl' }), null);
});
import assert from 'node:assert/strict';
import { parseCaptureBridge, claimBrowserCapture, captureTabMatches, captureSource, mergeCaptureSources, scopedMediaCookies, submitCapturedSources } from '../web-media-capture.mjs';

const id = 'd7430c5b-3b71-4db5-b7a6-1aa0cfec395d';
const page = 'https://video.example/watch?id=42#player';
const request = { id, client_id: 'extension-client', tab_id: 7, url: page, port: 7899, created: 1000 };
const cookie = { domain: '.example', path: '/media', name: 'session', value: 'secret', secure: true, httpOnly: true, hostOnly: false };

test('bridge requires the exact configured loopback URL and a fragment UUID', () => {
  assert.deepEqual(parseCaptureBridge(`http://127.0.0.1:7899/web-media-capture#${id}`, 7899), { id, port: 7899 });
  assert.deepEqual(parseCaptureBridge(`http://127.0.0.1:80/web-media-capture#${id}`, 80), { id, port: 80 });
  for (const url of [
    `http://localhost:7899/web-media-capture#${id}`, `https://127.0.0.1:7899/web-media-capture#${id}`,
    `http://127.0.0.1:7898/web-media-capture#${id}`, `http://127.0.0.1:7899/web-media-capture?x=1#${id}`,
    `http://user@127.0.0.1:7899/web-media-capture#${id}`, `http://127.0.0.1:7899/web-media-capture/#${id}`,
    'http://127.0.0.1:7899/web-media-capture#invalid',
  ]) assert.equal(parseCaptureBridge(url, 7899), null, url);
});

test('capture binding rejects other tabs, changed pages, private contexts and stale tickets', () => {
  assert.equal(captureTabMatches(request, { id: 7, url: page }, 2000), true);
  for (const tab of [{ id: 8, url: page }, { id: 7, url: `${page}other` }, { id: 7, url: page, incognito: true }]) {
    assert.equal(captureTabMatches(request, tab, 2000), false);
  }
  assert.equal(captureTabMatches(request, { id: 7, url: page }, 601001), false);
  assert.equal(captureTabMatches(request, { id: 7, url: page }, 999), false);
});

test('bridge claim uses the actual top-level sender tab and rechecks navigation after HTTP', async () => {
  const bridge = `http://127.0.0.1:7899/web-media-capture#${id}`;
  const sender = { id: 'extension', url: bridge, frameId: 0, tab: { id: 7 } };
  let calls = 0;
  const dependencies = {
    extensionId: 'extension', port: 7899, now: () => 2000, newClientId: () => 'client',
    getTab: async (tabId) => ({ id: tabId, url: bridge }), isAllowed: async () => true,
    claim: async (ticket, payload) => { calls++; assert.equal(ticket.id, id); assert.equal(payload.tab_id, 7); return { id, url: page, status: 'watching' }; },
    cancel: async () => {},
  };
  assert.deepEqual(await claimBrowserCapture(sender, dependencies), { id, client_id: 'client', tab_id: 7, url: page, port: 7899, created: 2000 });
  assert.equal(calls, 1);
  for (const invalid of [{ ...sender, frameId: 2 }, { ...sender, id: 'another-extension' }, { ...sender, url: page }]) {
    await assert.rejects(claimBrowserCapture(invalid, dependencies));
  }
  assert.equal(calls, 1, 'spoofed requests never claim tickets');
  await assert.rejects(claimBrowserCapture(sender, { ...dependencies, newClientId: () => 'extension:uuid' }));
  assert.equal(calls, 1, 'invalid client identifiers never reach the server');
  let read = 0, cancelled = 0;
  await assert.rejects(claimBrowserCapture(sender, { ...dependencies,
    getTab: async () => ({ id: 7, url: ++read === 1 ? bridge : page }),
    cancel: async () => { cancelled++; },
  }));
  assert.equal(cancelled, 1, 'navigation during claim revokes the claimed ticket');
});

test('real manifest/direct media preserve quality queries, exclude segments and ad noise', () => {
  assert.equal(captureSource({ url: 'https://cdn.example/stream', mime: 'application/dash+xml' }).protocol, 'dash');
  assert.equal(captureSource({ url: 'https://cdn.example/movie.m3u8?token=secret' }).protocol, 'hls');
  assert.equal(captureSource({ url: 'https://cdn.example/playlist', mime: 'audio/x-mpegurl' }).protocol, 'hls');
  assert.equal(captureSource({ url: 'https://cdn.example/movie.mp4' }).protocol, 'direct');
  for (const url of ['blob:https://video.example/abc', 'https://cdn.example/chunk.m4s', 'https://cdn.example/segment.ts',
    'https://cdn.example/segment.mp4', 'https://cdn.example/init.mp4', 'https://cdn.example/ad/video.mp4', 'https://doubleclick.net/movie.mp4',
    'http://127.0.0.1/movie.mp4', 'https://cdn.example/file.mp3']) {
    assert.equal(captureSource({ url, mime: 'video/mp4' }), null, url);
  }
  assert.equal(captureSource({ url: 'https://cdn.example/chunk', mime: 'video/mp2t' }), null);
  const sources = mergeCaptureSources([], [
    { url: 'https://cdn.example/movie.mp4?quality=720#one' },
    { url: 'https://cdn.example/movie.mp4?quality=720#two' },
    { url: 'https://cdn.example/movie.mp4?quality=1080' },
  ]);
  assert.equal(sources.length, 2);
  assert.match(sources[1].url, /quality=1080$/);
});

test('source permission revoked during cookie access prevents submission', async () => {
  let read = false;
  let submissions = 0;
  await assert.rejects(submitCapturedSources(request, [{ url: 'https://cdn.example/media/movie.mp4' }], {
    now: () => 2000,
    getTab: async () => ({ id: 7, url: page }),
    isAllowed: async (url) => url === page || !read,
    getCookies: async () => { read = true; return []; },
    submit: async () => { submissions++; },
  }));
  assert.equal(submissions, 0);
});

test('disabled embedded player cannot contribute sources or cookies', async () => {
  let cookieReads = 0;
  let submissions = 0;
  await submitCapturedSources(request, [{ url: 'https://cdn.example/media/movie.mp4', referrer: 'https://disabled.example/embed' }], {
    now: () => 2000, getTab: async () => ({ id: 7, url: page }),
    isAllowed: async (url) => !url.startsWith('https://disabled.example/'),
    getCookies: async () => { cookieReads++; return []; }, submit: async () => { submissions++; },
  });
  assert.equal(cookieReads, 0);
  assert.equal(submissions, 0);
});

test('actual network referrer and User-Agent survive ephemeral submission', async () => {
  let payload;
  await submitCapturedSources(request, [{ url: 'https://cdn.example/media/movie.mpd', referrer: 'https://player.example/embed/42', user_agent: 'Actual browser request' }], {
    now: () => 2000, getTab: async () => ({ id: 7, url: page }), isAllowed: async () => true,
    getCookies: async () => [], userAgent: 'Fallback user agent', submit: async (value) => { payload = value; },
  });
  assert.equal(payload.sources[0].referrer, 'https://player.example/embed/42');
  assert.equal(payload.sources[0].user_agent, 'Actual browser request');
  assert.deepEqual(payload.sources[0].cookies, []);
});

test('cookies obey host/domain, path boundary, HTTPS, expiry and payload bounds', () => {
  assert.deepEqual(scopedMediaCookies([
    cookie, { ...cookie, domain: 'other.example', hostOnly: true }, { ...cookie, path: '/media-other' },
    { ...cookie, expirationDate: 1 }, { ...cookie, domain: '.evilexample' },
  ], 'https://cdn.example/media/movie.m3u8', 2000), [
    { domain: '.example', path: '/media', name: 'session', value: 'secret', secure: true, http_only: true, host_only: false, expires: null },
  ]);
  assert.equal(scopedMediaCookies([cookie], 'http://cdn.example/media/movie.m3u8', 2000).length, 0);
  assert.equal(scopedMediaCookies([{ ...cookie, path: '/med' }], 'https://cdn.example/media/movie.m3u8', 2000).length, 0);
  assert.throws(() => scopedMediaCookies([{ ...cookie, value: 'bad\nvalue' }], 'https://cdn.example/media/file'), /Invalid/);
});

test('submission rechecks tab, settings and cookie container after async cookie reads', async () => {
  const calls = [];
  const source = { url: 'https://cdn.example/media/movie.m3u8', title: 'Video' };
  const dependencies = {
    now: () => 2000,
    getTab: async () => ({ id: 7, url: page, cookieStoreId: 'container-2' }),
    isAllowed: async () => true,
    getCookies: async (details) => { calls.push(['cookies', details]); return [cookie]; },
    submit: async (payload) => { calls.push(['submit', payload]); return { status: 'ready' }; },
    userAgent: 'Browser',
  };
  await submitCapturedSources(request, [source], dependencies);
  assert.deepEqual(calls[0], ['cookies', { url: source.url, storeId: 'container-2' }]);
  assert.equal(calls[1][1].tab_id, 7);
  assert.equal(calls[1][1].page_url, page);
  assert.equal(calls[1][1].sources[0].referrer, page);
  assert.equal(calls[1][1].sources[0].cookies[0].value, 'secret');
  for (const mode of ['navigation', 'container', 'disabled', 'other-tab', 'expired']) {
    let reads = 0;
    let submissions = 0;
    await assert.rejects(submitCapturedSources(request, [source], {
      ...dependencies,
      now: () => mode === 'expired' ? 700000 : 2000,
      getTab: async () => ({ id: mode === 'other-tab' ? 8 : 7, url: mode === 'navigation' && reads > 0 ? 'https://other.example/' : page,
        cookieStoreId: mode === 'container' && reads > 0 ? 'container-3' : 'container-2' }),
      isAllowed: async () => mode !== 'disabled',
      getCookies: async () => { reads++; return [cookie]; },
      submit: async () => { submissions++; },
    }));
    assert.equal(submissions, 0, mode);
    if (['disabled', 'other-tab', 'expired'].includes(mode)) assert.equal(reads, 0, mode);
  }
});
