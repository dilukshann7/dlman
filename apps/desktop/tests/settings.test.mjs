import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

// Keep the actual settings store; replace only the native command boundary.
const boundary = '__dlmanSettingsBoundary';
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === '@tauri-apps/api/core') {
      return {
        url: `data:text/javascript,${encodeURIComponent(`export const invoke = (...args) => globalThis.${boundary}(...args);`)}`,
        shortCircuit: true,
      };
    }
    return next(specifier, context);
  },
});

let serial = 0;
async function fixture(invoke = async () => {}) {
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const calls = [];
  globalThis[boundary] = (...args) => { calls.push(args); return invoke(...args); };
  const { useSettingsStore } = await import(`../src/stores/settings.ts?case=${serial++}`);
  useSettingsStore.setState({ loaded: true });
  return { store: useSettingsStore, calls };
}

test('new profiles enable launch at login by default', async () => {
  const { store } = await fixture();
  assert.equal(store.getState().settings.start_on_boot, true);
});

test('saving startup preference waits for native registration and submits once', async () => {
  let finish;
  const { store, calls } = await fixture(() => new Promise(resolve => { finish = resolve; }));
  const previous = store.getState().settings;
  const saving = store.getState().updateSettings({ start_on_boot: !previous.start_on_boot });
  for (let attempt = 0; !finish && attempt < 20; attempt++) {
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(typeof finish, 'function');
  assert.equal(store.getState().settings.start_on_boot, previous.start_on_boot);
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'update_settings');
  assert.equal(calls[0][1].settings.start_on_boot, !previous.start_on_boot);
  finish();
  await saving;
  assert.equal(store.getState().settings.start_on_boot, !previous.start_on_boot);
});

test('registration failure rejects save without changing the displayed preference', async () => {
  const { store } = await fixture(async () => { throw new Error('Startup registration denied'); });
  const previous = store.getState().settings;
  const saving = store.getState().updateSettings({ start_on_boot: !previous.start_on_boot });
  await assert.rejects(async () => { await saving; }, /Startup registration denied/);
  assert.equal(store.getState().settings.start_on_boot, previous.start_on_boot);
});

test('loading a saved opt-out preserves disabled startup', async () => {
  const { store } = await fixture(async command => {
    assert.equal(command, 'get_settings');
    return { ...store.getState().settings, start_on_boot: false };
  });
  await store.getState().loadFromBackend();
  assert.equal(store.getState().loaded, true);
  assert.equal(store.getState().settings.start_on_boot, false);
});

test('concurrent partial saves preserve both startup and appearance choices', async () => {
  const { store, calls } = await fixture();
  await Promise.all([
    store.getState().updateSettings({ start_on_boot: false }),
    store.getState().updateSettings({ font: 'inter' }),
  ]);
  assert.equal(store.getState().settings.start_on_boot, false);
  assert.equal(store.getState().settings.font, 'inter');
  assert.equal(calls.at(-1)[1].settings.start_on_boot, false);
});

test('defaults cannot be saved over a startup opt-out before hydration', async () => {
  const { store, calls } = await fixture();
  store.setState({ loaded: false });
  await assert.rejects(store.getState().updateSettings({ start_on_boot: true }), /loading/i);
  assert.equal(calls.length, 0);
});

test('automatic path resolution cannot persist defaults while settings are loading', async () => {
  const { store, calls } = await fixture();
  store.setState({ loaded: false });
  store.getState().setDefaultDownloadPath('C:/Users/Example/Downloads/DLMan');
  await store.getState().saveToBackend();
  assert.equal(calls.length, 0);
});
