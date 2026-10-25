import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import './dom-harness.mjs';

registerHooks({
  resolve(specifier, context, next) {
    if (specifier === './storage' && context.parentURL?.includes('automatic-media')) return {
      url: 'data:text/javascript,' + encodeURIComponent('export const settingsStorage={get:async()=>globalThis.__automaticTest.settings};export const disabledSitesStorage={isDisabled:async(host)=>globalThis.__automaticTest.settings.disabledSites.includes(host)};'),
      shortCircuit: true,
    };
    if (specifier === './api-client' && context.parentURL?.includes('automatic-media')) return {
      url: 'data:text/javascript,' + encodeURIComponent('export const getDlmanClient=()=>globalThis.__automaticTest.client;'),
      shortCircuit: true,
    };
    return next(specifier, context);
  },
});
const { handleAutomaticMedia, resetAutomaticMedia, forgetAutomaticMediaTab } = await import('../src/lib/automatic-media.ts');

function setup(context) {
  const original = new Map(['browser', '__automaticTest'].map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  context.after(() => { resetAutomaticMedia(); forgetAutomaticMediaTab(1); for (const [key, value] of original) {
    if (value) Object.defineProperty(globalThis, key, value); else delete globalThis[key];
  } });
  const calls = [], top = 'https://main.example/watch', frame = 'https://embed.example/player/42';
  const tab = { id: 1, active: true, url: top };
  globalThis.__automaticTest = { settings: { enabled: true, port: 7899, disabledSites: [] },
    client: { inspectWebMedia: async (url) => {
      calls.push(url); return { id: 'native', status: 'ready', preview: { url, identity: 'video', title: 'Video', variants: [] } };
    }, webMediaInspection: async () => assert.fail('Immediate preview needs no poll') } };
  globalThis.browser = { runtime: { id: 'extension' }, tabs: { get: async () => tab } };
  const sender = { id: 'extension', url: frame, frameId: 2, documentId: 'native-doc', tab: { id: 1, url: top } };
  const ready = { type: 'media-discovery-ready', document_id: 'content-doc' };
  const request = { type: 'web-media-auto-inspect', document_id: 'content-doc', player_id: 'player-1', url: frame };
  return { calls, tab, sender, ready, request, frame, top };
}

test('actual background adapter binds anonymous extraction to the sender frame and deduplicates requests', async (context) => {
  const state = setup(context);
  assert.equal((await handleAutomaticMedia(state.ready, state.sender)).enabled, true);
  assert.equal((await handleAutomaticMedia(state.request, state.sender)).status, 'ready');
  await handleAutomaticMedia(state.request, state.sender);
  assert.deepEqual(state.calls, [state.frame]);
  await assert.rejects(handleAutomaticMedia({ ...state.request, url: 'https://other.example/video' }, state.sender), /Invalid video page/);
  await handleAutomaticMedia({ ...state.ready, document_id: 'new-doc' }, state.sender);
  await assert.rejects(handleAutomaticMedia(state.request, state.sender), /changed/);
});

test('inactive tabs defer extraction; disabled parent sites and other extension senders cannot inspect', async (context) => {
  const state = setup(context);
  await handleAutomaticMedia(state.ready, state.sender);
  state.tab.active = false;
  await assert.rejects(handleAutomaticMedia(state.request, state.sender), (error) => error.deferred === true);
  state.tab.active = true;
  globalThis.__automaticTest.settings.disabledSites.push('main.example');
  assert.equal((await handleAutomaticMedia(state.ready, state.sender)).enabled, false);
  await assert.rejects(handleAutomaticMedia(state.request, state.sender), /disabled/);
  await assert.rejects(handleAutomaticMedia(state.request, { ...state.sender, id: 'other-extension' }), /Invalid video document/);
  assert.deepEqual(state.calls, []);
});

test('removed or offscreen player cannot revive queued work through an older inspection request', async (context) => {
  const state = setup(context);
  await handleAutomaticMedia(state.ready, state.sender);
  await handleAutomaticMedia({ ...state.request, type: 'media-discovery-player', eligible: false }, state.sender);
  await assert.rejects(handleAutomaticMedia(state.request, state.sender), /changed/);
  assert.deepEqual(state.calls, []);
  await handleAutomaticMedia({ ...state.request, type: 'media-discovery-player', eligible: true }, state.sender);
  assert.equal((await handleAutomaticMedia(state.request, state.sender)).status, 'ready');
});
