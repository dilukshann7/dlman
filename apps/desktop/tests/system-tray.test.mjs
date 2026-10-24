import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

const key = '__dlmanTrayBoundary';
const mocks = {
  '@tauri-apps/api/tray': `export const TrayIcon = { getById: (...args) => globalThis.${key}.getById(...args), new: (...args) => globalThis.${key}.create(...args) };`,
  '@tauri-apps/api/menu': `const factory = { new: async () => { globalThis.${key}.menus++; return {}; } }; export { factory as Menu, factory as MenuItem, factory as Submenu, factory as PredefinedMenuItem };`,
  '@tauri-apps/api/window': 'export const getCurrentWindow = () => ({});',
  '@tauri-apps/api/app': 'export const defaultWindowIcon = async () => ({});',
  '@tauri-apps/plugin-process': 'export const exit = async () => {};',
  '@/stores/ui': 'export const useUIStore = { getState: () => ({}) };',
  '@/stores/queues': 'export const useQueueStore = { getState: () => ({}) }; export const selectQueuesArray = () => [];',
  '@/lib/version': 'export const getAppVersion = async () => ({ current: "1.13.1" });',
  '@/i18n': 'export default { t: key => key, on: () => {} };',
};
registerHooks({
  resolve(specifier, context, next) {
    if (mocks[specifier]) return { url: `data:text/javascript,${encodeURIComponent(mocks[specifier])}`, shortCircuit: true };
    return next(specifier, context);
  },
});

let serial = 0;
async function fixture() {
  const state = { lookups: 0, creates: 0, menus: 0, attached: 0, tooltip: null, available: true };
  const nativeTray = {
    async setMenu() { state.attached++; },
    async setTooltip(value) { state.tooltip = value; },
  };
  Object.assign(state, {
    async getById(id) {
      state.lookups++;
      assert.equal(id, 'dlman-tray');
      await new Promise(resolve => setImmediate(resolve));
      return state.available ? nativeTray : null;
    },
    async create() { state.creates++; return nativeTray; },
  });
  globalThis[key] = state;
  const tray = await import(`../src/lib/system-tray.ts?case=${serial++}`);
  return { state, tray };
}

test('concurrent startup binds the one native tray once', async () => {
  const { state, tray } = await fixture();
  await Promise.all([tray.initSystemTray(), tray.initSystemTray(), tray.initSystemTray()]);
  assert.equal(state.lookups, 1);
  assert.equal(state.creates, 0);
  assert.equal(state.menus, 1);
  assert.equal(state.attached, 1);
  assert.equal(state.tooltip, 'tray.tooltip');
  await tray.initSystemTray();
  assert.equal(state.lookups, 1);
});

test('a missing backend tray rejects and can retry without creating another icon', async () => {
  const { state, tray } = await fixture();
  state.available = false;
  await assert.rejects(tray.initSystemTray(), /native tray/i);
  assert.equal(state.creates, 0);
  state.available = true;
  await tray.initSystemTray();
  assert.equal(state.attached, 1);
  assert.equal(state.creates, 0);
});
