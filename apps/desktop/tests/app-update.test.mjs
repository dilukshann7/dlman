import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

const storage = new Map();
globalThis.localStorage = {
  getItem: key => storage.get(key) ?? null,
  setItem: (key, value) => storage.set(key, value),
  removeItem: key => storage.delete(key),
};
globalThis.window = { __TAURI_INTERNALS__: {} };
let invoke;
let onProgress;
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === '@tauri-apps/api/core') return {
      url: 'data:text/javascript,export const invoke = (...args) => globalThis.__updateInvoke(...args);',
      shortCircuit: true,
    };
    if (specifier === '@tauri-apps/api/event') return {
      url: 'data:text/javascript,export const listen = (...args) => globalThis.__updateListen(...args);',
      shortCircuit: true,
    };
    return next(specifier, context);
  },
});
globalThis.__updateInvoke = (...args) => invoke(...args);
globalThis.__updateListen = async (_event, callback) => { onProgress = callback; return () => {}; };

let serial = 0;
const available = {
  hasUpdate: true, latestVersion: '1.14.0', currentVersion: '1.13.2',
  releaseUrl: 'https://github.com/dilukshann7/dlman/releases/tag/v1.14.0',
  supported: true, unsupportedReason: null,
};
async function fixture(handler = async () => available) {
  storage.clear();
  invoke = handler;
  const { useUpdateStore } = await import(`../src/stores/update.ts?case=${serial++}`);
  return useUpdateStore;
}

test('concurrent checks share a native request and never download automatically', async () => {
  const calls = [];
  let finish;
  const store = await fixture(command => {
    calls.push(command);
    return new Promise(resolve => { finish = resolve; });
  });
  const a = store.getState().checkForUpdates();
  const b = store.getState().checkForUpdates();
  finish(available);
  await Promise.all([a, b]);
  assert.deepEqual(calls, ['check_app_update']);
  assert.equal(store.getState().status, 'available');
});

test('failed checks show an error instead of up to date and can be retried', async () => {
  let fail = true;
  const store = await fixture(async () => {
    if (fail) throw new Error('Network unavailable');
    return { ...available, hasUpdate: false };
  });
  await store.getState().checkForUpdates();
  assert.equal(store.getState().status, 'error');
  assert.match(store.getState().error, /Network unavailable/);
  fail = false;
  await store.getState().checkForUpdates();
  assert.equal(store.getState().status, 'up-to-date');
});

test('download progress supports unknown totals and prevents duplicate downloads', async () => {
  let finish;
  const calls = [];
  const store = await fixture(async command => {
    calls.push(command);
    if (command === 'download_app_update') return new Promise(resolve => { finish = resolve; });
    return available;
  });
  await store.getState().checkForUpdates();
  const a = store.getState().downloadUpdate();
  await new Promise(resolve => setImmediate(resolve));
  const b = store.getState().downloadUpdate();
  onProgress({ payload: { downloaded: 1024, total: null } });
  assert.equal(store.getState().downloaded, 1024);
  assert.equal(store.getState().total, null);
  onProgress({ payload: { downloaded: 2048, total: 4096 } });
  assert.equal(store.getState().total, 4096);
  finish();
  await Promise.all([a, b]);
  assert.equal(calls.filter(command => command === 'download_app_update').length, 1);
  assert.equal(store.getState().status, 'ready');
});

test('download failure permits retry; successful payload survives an installer failure', async () => {
  let downloads = 0;
  const store = await fixture(async command => {
    if (command === 'check_app_update') return available;
    if (command === 'download_app_update' && downloads++ === 0) throw new Error('Download interrupted');
    if (command === 'install_app_update') throw new Error('Installer denied');
    return true;
  });
  await store.getState().checkForUpdates();
  await store.getState().downloadUpdate();
  assert.equal(store.getState().failedAction, 'download');
  await store.getState().downloadUpdate();
  assert.equal(store.getState().status, 'ready');
  await store.getState().installUpdate();
  assert.equal(store.getState().status, 'ready');
  assert.match(store.getState().error, /Installer denied/);
  assert.equal(downloads, 2);
});

test('installation rechecks native readiness and never interrupts active work', async () => {
  const calls = [];
  const store = await fixture(async command => {
    calls.push(command);
    if (command === 'check_app_update') return available;
    if (command === 'can_install_app_update') return false;
  });
  await store.getState().checkForUpdates();
  await store.getState().downloadUpdate();
  await store.getState().installUpdate();
  assert.equal(calls.includes('install_app_update'), false);
  assert.equal(store.getState().status, 'ready');
  assert.equal(store.getState().canInstall, false);
});

test('dismissal affects notifications only; manual checks and newer versions remain visible', async () => {
  let info = available;
  const store = await fixture(async () => info);
  await store.getState().checkForUpdates();
  store.getState().dismissUpdate();
  assert.equal(store.getState().shouldShowNotification(), false);
  await store.getState().checkForUpdates();
  assert.equal(store.getState().updateInfo.latestVersion, '1.14.0');
  info = { ...available, latestVersion: '1.15.0' };
  await store.getState().checkForUpdates();
  assert.equal(store.getState().shouldShowNotification(), true);
});

test('unsupported installations retain release information without invoking the downloader', async () => {
  const calls = [];
  const store = await fixture(async command => {
    calls.push(command);
    return { ...available, supported: false, unsupportedReason: 'package-manager' };
  });
  await store.getState().checkForUpdates();
  await store.getState().downloadUpdate();
  assert.deepEqual(calls, ['check_app_update']);
  assert.equal(store.getState().updateInfo.unsupportedReason, 'package-manager');
});
