import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

// Mock the native API and notifications; execute the actual Zustand store.
const boundaryKey = '__dlmanRefreshTestBoundary';
const mocks = {
  '@tauri-apps/api/core': `export const invoke = (...args) => globalThis.${boundaryKey}.invoke(...args);`,
  sonner: `export const toast = { success: value => globalThis.${boundaryKey}.toasts.push(value), error: value => globalThis.${boundaryKey}.toasts.push(value) };`,
  '@/i18n': 'export default { t: key => key };',
};
registerHooks({
  resolve(specifier, context, next) {
    if (mocks[specifier]) return { url: `data:text/javascript,${encodeURIComponent(mocks[specifier])}`, shortCircuit: true };
    return next(specifier, context);
  },
});

let serial = 0;
const download = { id: 'job', url: 'https://example.com/file', status: 'paused' };
function session() {
  return { session_id: 'session', download_id: 'job', expires_at: Date.now() + 600000, candidate: null };
}
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}
async function harness(implementation) {
  const calls = [];
  const toasts = [];
  globalThis[boundaryKey] = {
    toasts,
    invoke(command, args) { calls.push([command, args]); return implementation(command, args); },
  };
  const module = await import(`../src/stores/download-refresh.ts?test=${++serial}`);
  return { get: module.useDownloadRefresh.getState, calls, toasts };
}

test('browser capture requires explicit confirmation and rejects stale-session offers', async () => {
  const original = session();
  const state = await harness(async command => command === 'begin_download_refresh' ? original : { started: true });
  await state.get().open(download);
  state.get().offer({ ...original, session_id: 'other', candidate: { candidate_id: 'wrong' } });
  assert.equal(state.get().session.candidate, null);
  state.get().offer({ ...original, candidate: { candidate_id: 'candidate', display_url: 'https://example.com/file' } });
  assert.equal(state.calls.length, 1);
  await state.get().verify(undefined, 'candidate');
  assert.deepEqual(state.calls[1], ['refresh_download_address', { session_id: 'session', url: null, candidate_id: 'candidate' }]);
  assert.equal(state.get().download, null);
});

test('verification rejects duplicate submission and closes only after success', async () => {
  const pending = deferred();
  const state = await harness(command => command === 'begin_download_refresh' ? Promise.resolve(session()) : pending.promise);
  await state.get().open(download);
  const first = state.get().verify('https://example.com/fresh');
  await state.get().verify('https://example.com/duplicate');
  assert.equal(state.calls.length, 2);
  assert.equal(state.get().busy, true);
  pending.resolve({ started: true });
  await first;
  assert.equal(state.get().download, null);
});

test('closing while begin is pending cancels the returned backend session', async () => {
  const pending = deferred();
  const state = await harness(command => command === 'begin_download_refresh' ? pending.promise : Promise.resolve());
  const opening = state.get().open(download);
  await state.get().close();
  pending.resolve(session());
  await opening;
  assert.equal(state.get().download, null);
  assert.equal(state.calls.at(-1)[0], 'cancel_download_refresh');
});

test('verification failure leaves the dialog ready for another attempt', async () => {
  const state = await harness(async command => {
    if (command === 'begin_download_refresh') return session();
    throw new Error('Replacement bytes do not match');
  });
  await state.get().open(download);
  await state.get().verify('https://example.com/wrong');
  assert.equal(state.get().busy, false);
  assert.match(state.get().error, /Replacement bytes do not match/);
  assert.equal(state.get().download.id, 'job');
});

test('Add as new download releases the capture and closes refresh', async () => {
  const original = session();
  const state = await harness(async command => command === 'begin_download_refresh' ? original : undefined);
  await state.get().open(download);
  state.get().offer({ ...original, candidate: { candidate_id: 'unrelated' } });
  await state.get().releaseCandidate();
  assert.deepEqual(state.calls.at(-1), ['release_refresh_candidate', { session_id: 'session', candidate_id: 'unrelated' }]);
  assert.equal(state.get().session, null);
});

test('saved replacement with a failed resume reports the error without retrying a spent session', async () => {
  const state = await harness(async command => command === 'begin_download_refresh' ? session() : { started: false, error: 'Address saved; retry Resume.' });
  await state.get().open(download);
  await state.get().verify('https://example.com/fresh');
  assert.equal(state.get().session, null);
  assert.deepEqual(state.toasts, ['Address saved; retry Resume.']);
});

test('a completed native reply cannot reopen a cancelled refresh', async () => {
  const pending = deferred();
  const state = await harness(command => {
    if (command === 'begin_download_refresh') return Promise.resolve(session());
    if (command === 'refresh_download_address') return pending.promise;
    return Promise.resolve();
  });
  await state.get().open(download);
  const verifying = state.get().verify('https://example.com/fresh');
  await state.get().close();
  pending.resolve({ started: true });
  await verifying;
  assert.equal(state.get().session, null);
  assert.deepEqual(state.toasts, []);
});

test('a browser capture arriving before begin resolves remains visible', async () => {
  const pending = deferred();
  const state = await harness(command => command === 'begin_download_refresh' ? pending.promise : Promise.resolve());
  const original = session();
  const opening = state.get().open(download);
  state.get().offer({ ...original, candidate: { candidate_id: 'early', display_url: 'https://example.com/file' } });
  pending.resolve(original);
  await opening;
  assert.equal(state.get().session.candidate?.candidate_id, 'early');
});
