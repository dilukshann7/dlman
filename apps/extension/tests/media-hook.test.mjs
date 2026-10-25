import test from 'node:test';
import assert from 'node:assert/strict';
import { installMediaHook } from '../src/lib/media-hook.ts';

test('hook replays early confirmed detections after readiness and keeps signed variants distinct', async (context) => {
  const names = ['window', 'location', 'XMLHttpRequest'];
  const saved = names.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  const originalArrayBuffer = Response.prototype.arrayBuffer, originalClone = Response.prototype.clone;
  context.after(() => { for (const [name, value] of saved) {
    if (value) Object.defineProperty(globalThis, name, value); else delete globalThis[name];
  } Response.prototype.arrayBuffer = originalArrayBuffer; Response.prototype.clone = originalClone; });
  const messages = [], handlers = new Map();
  globalThis.location = new URL('https://video.example/watch');
  globalThis.XMLHttpRequest = class { open() {} send() {} };
  globalThis.window = {
    fetch: async (url) => new Response('video', { status: String(url).includes('missing') ? 404 : 200, headers: { 'content-type': 'video/mp4' } }),
    postMessage: (data) => messages.push(data),
    addEventListener: (name, handler) => handlers.set(name, handler),
  };
  installMediaHook();
  await window.fetch('https://cdn.example/main.mp4?quality=720');
  await window.fetch('https://cdn.example/main.mp4?quality=1080');
  await window.fetch('https://cdn.example/ads/preroll.mp4');
  await window.fetch('https://cdn.example/missing.mp4');
  messages.length = 0;
  handlers.get('message')({ source: window, data: { source: 'dlman-content', type: 'media-hook-ready' } });
  assert.deepEqual(messages.map((message) => message.url), [
    'https://cdn.example/main.mp4?quality=720', 'https://cdn.example/main.mp4?quality=1080',
  ]);
});

test('HLS network fallback associates actual appended buffers with their owning MSE blob', async (context) => {
  const names = ['window', 'location', 'XMLHttpRequest', 'MediaSource', 'SourceBuffer'];
  const saved = names.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  const originals = { arrayBuffer: Response.prototype.arrayBuffer, clone: Response.prototype.clone, create: URL.createObjectURL };
  context.after(() => {
    for (const [name, value] of saved) { if (value) Object.defineProperty(globalThis, name, value); else delete globalThis[name]; }
    Response.prototype.arrayBuffer = originals.arrayBuffer; Response.prototype.clone = originals.clone;
    URL.createObjectURL = originals.create;
  });
  const messages = [];
  globalThis.location = new URL('https://video.example/watch');
  globalThis.XMLHttpRequest = class { open() {} send() {} };
  globalThis.SourceBuffer = class { appendBuffer() { return 'original-result'; } };
  globalThis.MediaSource = class { addSourceBuffer() { return new SourceBuffer(); } };
  URL.createObjectURL = () => 'blob:https://video.example/owner';
  globalThis.window = {
    fetch: async (url) => new Response(String(url).endsWith('master.m3u8') ? '#EXTM3U\nquality.m3u8'
      : String(url).endsWith('quality.m3u8') ? '#EXTM3U\nsegment.ts' : new Uint8Array([1, 2, 3]),
      { headers: { 'content-type': String(url).endsWith('.m3u8') ? 'application/vnd.apple.mpegurl' : 'video/mp2t' } }),
    postMessage: (message) => messages.push(message), addEventListener: () => {},
  };
  installMediaHook();
  await (await window.fetch('https://cdn.example/master.m3u8')).text();
  await new Promise(setImmediate);
  await (await window.fetch('https://cdn.example/quality.m3u8')).text();
  await new Promise(setImmediate);
  const bytes = await (await window.fetch('https://cdn.example/segment.ts')).arrayBuffer();
  const source = new MediaSource();
  const blobUrl = URL.createObjectURL(source);
  assert.equal(source.addSourceBuffer('video/mp4').appendBuffer(new Uint8Array(bytes)), 'original-result');
  assert.deepEqual(messages.filter((message) => message.blobUrl).map(({ url, blobUrl: owner }) => ({ url, owner })), [
    { url: 'https://cdn.example/master.m3u8', owner: blobUrl },
  ]);
});
