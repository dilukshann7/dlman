import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

const boundaryKey = '__dlmanWebCaptureTest';
const mocks = {
  react: `export const useState = (...args) => globalThis.${boundaryKey}.useState(...args);
    export const useRef = (...args) => globalThis.${boundaryKey}.useRef(...args);
    export const useCallback = (...args) => globalThis.${boundaryKey}.useCallback(...args);
    export const useEffect = (...args) => globalThis.${boundaryKey}.useEffect(...args);`,
  '@tauri-apps/api/core': `export const invoke = (...args) => globalThis.${boundaryKey}.invoke(...args);`,
  '@tauri-apps/plugin-shell': `export const open = (...args) => globalThis.${boundaryKey}.open(...args);`,
};
registerHooks({
  resolve(specifier, context, next) {
    if (mocks[specifier]) return { url: `data:text/javascript,${encodeURIComponent(mocks[specifier])}`, shortCircuit: true };
    if (specifier === './useWebMediaCapture') return { url: new URL('./useWebMediaCapture.ts', context.parentURL).href, shortCircuit: true };
    return next(specifier, context);
  },
});

const { useWebMediaCapture } = await import('../src/hooks/useWebMediaCapture.ts');
const { useWebMediaInspection } = await import('../src/hooks/useWebMediaInspection.ts');
const { webMediaExtensionContext } = await import('../src/lib/web-media-context.ts');
function deferred() {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
const capture = (id, sources = []) => ({ id, url: 'https://example.com/watch', status: 'waiting', sources,
  bridge_url: `http://127.0.0.1:5000/web-media-capture#${id}` });

// Small effect runner keeps the actual hook's state, dependency and cleanup
// behavior testable without starting a browser, desktop app or rendering server.
function harness(implementation, hook = useWebMediaCapture, initialProps = ['https://example.com/watch', true]) {
  const cells = [], calls = [], opened = [];
  let cursor = 0, effects = [], dirty = true, value;
  let props = initialProps;
  const same = (a, b) => a && b && a.length === b.length && a.every((item, index) => Object.is(item, b[index]));
  const boundary = {
    invoke(command, args) { calls.push([command, args]); return Promise.resolve().then(() => implementation(command, args)); },
    open(url) { opened.push(url); return Promise.resolve(); },
    useState(initial) {
      const index = cursor++;
      cells[index] ||= { value: typeof initial === 'function' ? initial() : initial };
      return [cells[index].value, (next) => {
        const nextValue = typeof next === 'function' ? next(cells[index].value) : next;
        if (!Object.is(nextValue, cells[index].value)) { cells[index].value = nextValue; dirty = true; }
      }];
    },
    useRef(initial) { const index = cursor++; cells[index] ||= { current: initial }; return cells[index]; },
    useCallback(callback, deps) {
      const index = cursor++;
      if (!same(cells[index]?.deps, deps)) cells[index] = { deps, callback };
      return cells[index].callback;
    },
    useEffect(callback, deps) {
      const index = cursor++;
      if (!same(cells[index]?.deps, deps)) effects.push(() => {
        cells[index]?.cleanup?.();
        cells[index] = { deps, cleanup: callback() };
      });
    },
  };
  globalThis[boundaryKey] = boundary;
  function render(nextProps = props) {
    props = nextProps;
    do {
      dirty = false; cursor = 0; effects = [];
      value = hook(...props);
      for (const effect of effects) effect();
    } while (dirty);
    return value;
  }
  return { render, calls, opened, unmount() { for (const cell of cells) cell.cleanup?.(); } };
}

test('closing while capture begins cancels its ticket without opening a browser', async () => {
  const pending = deferred();
  const state = harness((command) => command === 'begin_web_media_capture' ? pending.promise : undefined);
  const opening = state.render().begin();
  state.render(['https://example.com/watch', false]);
  pending.resolve(capture('old'));
  await opening;
  assert.deepEqual(state.opened, []);
  assert.ok(state.calls.some(([command, args]) => command === 'cancel_web_media_capture' && args.id === 'old'));
  state.unmount();
});

test('an older failed begin cannot clear a newer opening state', async () => {
  const first = deferred(), second = deferred();
  let starts = 0;
  const state = harness((command) => command === 'begin_web_media_capture' ? (++starts === 1 ? first.promise : second.promise) : undefined);
  const old = state.render().begin();
  await Promise.resolve();
  state.render().cancel();
  const fresh = state.render().begin();
  first.reject(new Error('Old request failed'));
  await old;
  assert.equal(state.render().waiting, true);
  assert.equal(state.render().error, undefined);
  state.render().cancel();
  second.resolve(capture('new'));
  await fresh;
  state.unmount();
});

test('late poll after a URL change never selects a previous video', async () => {
  const polling = deferred();
  const state = harness((command) => command === 'begin_web_media_capture' ? capture('old')
    : command === 'get_web_media_capture' ? polling.promise : undefined);
  await state.render().begin();
  state.render(['https://other.example.com/watch', true]);
  polling.resolve({ ...capture('old', [{ id: 'stale', url: 'https://media.example.com/old.mp4', protocol: 'direct' }]), status: 'ready' });
  await Promise.resolve(); await Promise.resolve();
  assert.equal(state.render().sourceId, undefined);
  assert.equal(state.render().capture, undefined);
  state.unmount();
});

test('ready capture retains every source and selects a single source automatically', async () => {
  const sources = [{ id: 'one', url: 'https://media.example.com/video.mp4', protocol: 'direct' }];
  const state = harness((command) => command === 'begin_web_media_capture' ? capture('ticket')
    : command === 'get_web_media_capture' ? { ...capture('ticket', sources), status: 'ready' } : undefined);
  await state.render().begin();
  await Promise.resolve(); await Promise.resolve();
  assert.equal(state.render().sourceId, 'one');
  assert.deepEqual(state.render().capture.sources, sources);
  state.unmount();
  assert.ok(state.calls.some(([command]) => command === 'cancel_web_media_capture'));
});

test('later sources revoke an automatic sole-source choice; explicit choices survive polling', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let sources = [{ id: 'one', url: 'https://media.example.com/one.mp4', protocol: 'direct' }];
  const state = harness((command) => command === 'begin_web_media_capture' ? capture('ticket')
    : command === 'get_web_media_capture' ? { ...capture('ticket', sources), status: 'ready' } : undefined);
  const settle = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
  await state.render().begin();
  await settle();
  assert.equal(state.render().sourceId, 'one');
  sources = [...sources, { id: 'two', url: 'https://media.example.com/two.m3u8', protocol: 'hls' }];
  context.mock.timers.tick(1000);
  await settle();
  assert.equal(state.render().sourceId, undefined);
  state.render().chooseSource('two');
  context.mock.timers.tick(1000);
  await settle();
  assert.equal(state.render().sourceId, 'two');
  assert.equal(state.render().waiting, false);
  const polls = state.calls.filter(([command]) => command === 'get_web_media_capture').length;
  state.unmount();
  context.mock.timers.tick(10_000);
  await settle();
  assert.equal(state.calls.filter(([command]) => command === 'get_web_media_capture').length, polls);
});

test('ordinary cookie-authenticated extension files remain in the ordinary file flow', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const url = 'https://files.example.com/private/archive.zip';
  const extensionContext = { master_url: url, cookies: 'session=private', referrer: 'https://files.example.com/account', expectVideo: false };
  const state = harness(() => ({ id: 'file', status: 'file' }), useWebMediaInspection, [url, true, extensionContext]);
  state.render();
  context.mock.timers.tick(500);
  for (let turn = 0; turn < 8; turn++) await Promise.resolve();
  assert.equal(state.render().file, true);
  assert.equal(state.render().error, undefined);
  assert.equal(state.render().selection, undefined);
  assert.deepEqual(state.calls[0], ['inspect_web_media', { url, referrer: extensionContext.referrer, cookies: extensionContext.cookies, page_title: undefined }]);
  state.unmount();
});

test('extension manifests never fall back to ordinary file transfer', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const url = 'https://media.example.com/private/master.m3u8';
  const extensionContext = { master_url: url, cookies: 'session=private', expectVideo: true };
  const state = harness(() => ({ id: 'file', status: 'file' }), useWebMediaInspection, [url, true, extensionContext]);
  state.render();
  context.mock.timers.tick(500);
  for (let turn = 0; turn < 8; turn++) await Promise.resolve();
  assert.equal(state.render().file, undefined);
  assert.match(state.render().error, /No downloadable video/);
  state.unmount();
});

test('browser cookies and referrer bind to the exact original intake URL', () => {
  const original = 'https://files.example.com/private/archive.zip';
  const intake = { cookies: 'session=private', sourcePageUrl: 'https://files.example.com/account' };
  assert.deepEqual(webMediaExtensionContext(original, original, intake), {
    master_url: original, referrer: intake.sourcePageUrl, cookies: intake.cookies,
    page_title: undefined, expectVideo: false,
  });
  assert.equal(webMediaExtensionContext('https://other.example.com/archive.zip', original, intake), undefined);
  assert.equal(webMediaExtensionContext(`${original}?changed=1`, original, intake), undefined);
  assert.equal(webMediaExtensionContext(original, undefined, intake), undefined);
  const stream = webMediaExtensionContext(original, original, { ...intake,
    media: { protocol: 'hls', master_url: 'https://cdn.example.com/master.m3u8', referrer: 'https://files.example.com/watch' } });
  assert.equal(stream.page_url, intake.sourcePageUrl);
  assert.equal(stream.expectVideo, true);
});

test('extension streams inspect and transfer the original webpage with a transient manifest source', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const master = 'https://cdn.example.com/signed/master.m3u8?token=secret';
  const page = 'https://example.com/watch/123';
  const extensionContext = { master_url: master, page_url: page, referrer: page, cookies: 'session=private', expectVideo: true };
  const preview = { url: page, identity: 'page-video', title: 'Video', variants: [
    { id: 'source:1080', quality: '1080p', container: 'mp4', audio: 'AAC', output_mode: 'video' },
  ] };
  const state = harness((command) => command === 'inspect_web_media' ? { id: 'video', status: 'ready', preview }
    : command === 'begin_web_media_capture' ? { ...capture('ticket'), url: page } : { ...capture('ticket'), url: page, status: 'ready', sources: [] },
  useWebMediaInspection, [master, true, extensionContext]);
  state.render();
  context.mock.timers.tick(500);
  for (let turn = 0; turn < 8; turn++) await Promise.resolve();
  assert.equal(state.render().transferUrl, page);
  assert.deepEqual(state.calls[0], ['inspect_web_media', {
    url: page, master_url: master, referrer: page, cookies: extensionContext.cookies, page_title: undefined,
  }]);
  await state.render().capture.begin();
  assert.deepEqual(state.calls.find(([command]) => command === 'begin_web_media_capture'), ['begin_web_media_capture', { url: page }]);
  assert.deepEqual(state.opened, ['http://127.0.0.1:5000/web-media-capture#ticket']);
  state.unmount();
});

test('extension webpage quality survives reinspection and is revalidated before submission', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const page = 'https://example.com/watch/123';
  const preview = { url: page, identity: 'video-123', title: 'Video', variants: [
    { id: '1080', quality: '1080p', container: 'mp4', audio: 'AAC', output_mode: 'video' },
    { id: '720', quality: '720p', container: 'mp4', audio: 'AAC', output_mode: 'video' },
  ] };
  const selected = { identity: preview.identity, format_id: '720', quality: '720p', container: 'mp4', output_mode: 'video' };
  let available = true;
  const state = harness((command) => command === 'inspect_web_media'
    ? { id: 'fresh', status: 'ready', preview: { ...preview, variants: available ? preview.variants : preview.variants.slice(0, 1) } }
    : undefined, useWebMediaInspection, [page, true, undefined, selected]);
  state.render(); context.mock.timers.tick(500);
  for (let turn = 0; turn < 12; turn++) await Promise.resolve();
  assert.deepEqual(state.render().selection, selected);
  assert.equal(await state.render().refreshSelection(), 'fresh');
  available = false;
  await assert.rejects(state.render().refreshSelection(), /Selected quality is no longer available/);
  state.unmount();
});

test('an unavailable extension quality reports an error instead of silently selecting another format', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const preview = { url: 'https://example.com/watch', identity: 'video', title: 'Video', variants: [
    { id: '1080', quality: '1080p', container: 'mp4', output_mode: 'video' },
  ] };
  const selected = { identity: 'different-video', format_id: '720', quality: '720p', container: 'mp4', output_mode: 'video' };
  const state = harness(() => ({ id: 'id', status: 'ready', preview }), useWebMediaInspection,
    [preview.url, true, undefined, selected]);
  state.render(); context.mock.timers.tick(500);
  for (let turn = 0; turn < 12; turn++) await Promise.resolve();
  assert.match(state.render().error, /Selected quality is no longer available/);
  assert.equal(state.render().selection, undefined);
  state.render().retry();
  state.render(); context.mock.timers.tick(500);
  for (let turn = 0; turn < 12; turn++) await Promise.resolve();
  assert.equal(state.render().selection.format_id, '1080');
  state.unmount();
});

test('HLS rendition quality is selected from the full master so companion audio remains available', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const page = 'https://example.com/watch';
  const master = 'https://cdn.example.com/master.m3u8';
  const preview = { url: page, identity: 'master', title: 'Video', variants: [
    { id: '1080+audio', quality: '1080p', fps: 60, container: 'mp4', audio: 'AAC', output_mode: 'video' },
    { id: '720+audio', quality: '720p', fps: 30, container: 'mp4', audio: 'AAC', output_mode: 'video' },
  ] };
  const intake = webMediaExtensionContext(master, master, { sourcePageUrl: page, media: {
    protocol: 'hls', master_url: master, media_quality: { height: 720, fps: 30, label: '720p' },
  } });
  const state = harness(() => ({ id: 'master-preview', status: 'ready', preview }), useWebMediaInspection, [master, true, intake]);
  state.render(); context.mock.timers.tick(500);
  for (let turn = 0; turn < 12; turn++) await Promise.resolve();
  assert.equal(state.calls[0][1].master_url, master);
  assert.equal(state.render().selection.format_id, '720+audio');
  assert.equal(await state.render().refreshSelection(), 'master-preview');
  state.unmount();
});
