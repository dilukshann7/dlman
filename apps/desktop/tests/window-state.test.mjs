import test from 'node:test';
import assert from 'node:assert/strict';
import { rememberWindowState } from '../src/lib/window-state.ts';

const key = 'dlman-window-state';

function storageWith(value) {
  const values = new Map(value === undefined ? [] : [[key, value]]);
  return {
    getItem: (name) => values.get(name) ?? null,
    setItem: (name, value) => values.set(name, value),
    state: () => JSON.parse(values.get(key)),
  };
}

// The native window is the external boundary; persistence and restore use real code.
function nativeWindow() {
  let resized;
  const state = {
    width: 1200, height: 800, scale: 1,
    minimized: false, maximized: false, fullscreen: false,
  };
  return {
    state,
    restored: [],
    centered: false,
    innerSize: async () => ({ width: state.width, height: state.height }),
    scaleFactor: async () => state.scale,
    isMinimized: async () => state.minimized,
    isMaximized: async () => state.maximized,
    isFullscreen: async () => state.fullscreen,
    async setSize(size) {
      this.restored.push({ width: size.width, height: size.height, type: size.type });
      state.width = size.width * state.scale;
      state.height = size.height * state.scale;
    },
    async maximize() { state.maximized = true; },
    async center() { this.centered = true; },
    async onResized(handler) {
      resized = handler;
      return () => { resized = undefined; };
    },
    async resize(width, height, flags = {}) {
      Object.assign(state, { width, height }, flags);
      await resized?.({ payload: { width, height } });
    },
  };
}

test('reopening restores the last resized client size', async () => {
  const storage = storageWith();
  const first = nativeWindow();
  const stop = await rememberWindowState(first, storage);
  await first.resize(1040, 720);
  stop();

  const reopened = nativeWindow();
  await rememberWindowState(reopened, storage);
  assert.deepEqual(reopened.restored, [{ width: 1040, height: 720, type: 'Logical' }]);
  assert.equal(reopened.centered, true);
});

test('DPI changes preserve the same logical window size', async () => {
  const storage = storageWith();
  const first = nativeWindow();
  first.state.scale = 1.5;
  await rememberWindowState(first, storage);
  await first.resize(1560, 1080);
  assert.deepEqual(storage.state(), { width: 1040, height: 720, maximized: false });

  const reopened = nativeWindow();
  reopened.state.scale = 2;
  await rememberWindowState(reopened, storage);
  assert.equal(reopened.state.width, 2080);
  assert.equal(reopened.state.height, 1440);
});

test('maximizing remembers the normal size and restores maximized on restart', async () => {
  const storage = storageWith();
  const first = nativeWindow();
  await rememberWindowState(first, storage);
  await first.resize(1040, 720);
  await first.resize(1920, 1080, { maximized: true });
  assert.deepEqual(storage.state(), { width: 1040, height: 720, maximized: true });

  const reopened = nativeWindow();
  await rememberWindowState(reopened, storage);
  assert.deepEqual(reopened.restored, [{ width: 1040, height: 720, type: 'Logical' }]);
  assert.equal(reopened.state.maximized, true);
});

test('restoring from maximized saves the normal state again', async () => {
  const storage = storageWith(JSON.stringify({ width: 1040, height: 720, maximized: true }));
  const window = nativeWindow();
  await rememberWindowState(window, storage);
  await window.resize(1040, 720, { maximized: false });
  assert.deepEqual(storage.state(), { width: 1040, height: 720, maximized: false });
});

test('minimizing, fullscreen and zero-sized events cannot overwrite saved dimensions', async () => {
  const storage = storageWith();
  const window = nativeWindow();
  await rememberWindowState(window, storage);
  await window.resize(1040, 720);
  await window.resize(0, 0, { minimized: true });
  await window.resize(1920, 1080, { minimized: false, fullscreen: true });
  await window.resize(0, 0, { fullscreen: false });
  assert.deepEqual(storage.state(), { width: 1040, height: 720, maximized: false });
});

test('invalid saved state keeps native defaults instead of applying bad sizes', async () => {
  for (const value of ['broken', 'null', '{}',
    '{"width":0,"height":0,"maximized":false}',
    '{"width":1200,"height":800,"maximized":"false"}',
    '{"width":1e400,"height":800,"maximized":false}',
  ]) {
    const storage = storageWith(value);
    const window = nativeWindow();
    await rememberWindowState(window, storage);
    assert.deepEqual(window.restored, []);
    assert.deepEqual(storage.state(), { width: 1200, height: 800, maximized: false });
  }
});

test('an older asynchronous size read cannot overwrite a newer resize', async () => {
  const storage = storageWith();
  const window = nativeWindow();
  await rememberWindowState(window, storage);
  let release;
  const readSize = window.innerSize;
  window.innerSize = () => new Promise((resolve) => { release = resolve; });
  const older = window.resize(1000, 700);
  // Wait until the first save reaches the delayed native size query.
  for (let i = 0; !release && i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
  assert.equal(typeof release, 'function');
  window.innerSize = readSize;
  await window.resize(1100, 750);
  release({ width: 1000, height: 700 });
  await older;
  assert.deepEqual(storage.state(), { width: 1100, height: 750, maximized: false });
});

test('cleanup unsubscribes and discards size queries still in flight', async () => {
  const storage = storageWith();
  const window = nativeWindow();
  const stop = await rememberWindowState(window, storage);
  let release;
  window.innerSize = () => new Promise((resolve) => { release = resolve; });
  const pending = window.resize(1040, 720);
  for (let i = 0; !release && i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
  assert.equal(typeof release, 'function');
  stop();
  release({ width: 1040, height: 720 });
  await pending;
  await window.resize(1100, 750);
  assert.deepEqual(storage.state(), { width: 1200, height: 800, maximized: false });
});
