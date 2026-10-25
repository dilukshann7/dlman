import test from 'node:test';
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import { domHarness } from './dom-harness.mjs';

registerHooks({
  resolve(specifier, context, next) {
    if (specifier === 'wxt/sandbox') {
      return { url: 'data:text/javascript,export const defineContentScript = config => config;', shortCircuit: true };
    }
    return next(specifier, context);
  },
});
const { default: content } = await import('../src/entrypoints/web-media-capture.ts');

test('claimed content script captures unloaded HTML5 qualities and inline KVS configuration', async (context) => {
  const dom = domHarness(context, '<video preload="none"><source src="https://cdn.example/get_file/42_720p.mp4/" type="video/mp4" label="720p"><source src="https://cdn.example/get_file/42_480p.mp4/" type="video/mp4" label="480p"></video><script>var flashvars = {video_url:"https://cdn.example/get_file/42_1080p.mp4/?v-acctoken=temporary",video_url_text:"1080p"};</script>');
  dom.document.title = 'Example clip';
  const messages = [];
  browser.runtime.sendMessage = async (message) => { messages.push(message); return { active: true }; };
  await content.main();
  dom.tick();
  const submitted = messages.find((message) => message.type === 'web-media-capture-sources');
  assert.deepEqual(submitted.sources.map((candidate) => candidate.title), [
    'Example clip · 720p', 'Example clip · 480p', 'Example clip · 1080p',
  ]);
  assert.equal(submitted.sources[2].url, 'https://cdn.example/get_file/42_1080p.mp4/?v-acctoken=temporary');
  dom.window.dispatchEvent(new dom.window.Event('pagehide'));
});

test('capture observes known player configuration in an open shadow root and excludes ad containers', async (context) => {
  const dom = domHarness(context, '<div id="host"></div><div class="ad-container"><video src="/ads/preroll.mp4"></video></div>');
  const shadow = dom.document.querySelector('#host').attachShadow({ mode: 'open' });
  shadow.innerHTML = '<div id="jw" class="jwplayer"></div><script>jwplayer("jw").setup({sources:[{file:"https://cdn.example/main.mp4",label:"720p"}]});</script>';
  const messages = [];
  browser.runtime.sendMessage = async (message) => { messages.push(message); return { active: true }; };
  await content.main(); dom.tick();
  const submitted = messages.find((message) => message.type === 'web-media-capture-sources');
  assert.deepEqual(submitted.sources.map((source) => source.url), ['https://cdn.example/main.mp4']);
  dom.window.dispatchEvent(new dom.window.Event('pagehide'));
});
