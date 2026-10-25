import test from 'node:test';
import assert from 'node:assert/strict';
import { domHarness } from './dom-harness.mjs';
const { VideoDetector } = await import('../src/lib/video-detector.ts');
const { VideoOverlayManager } = await import('../src/lib/video-overlay.ts');
const { parseMasterPlaylist } = await import('../src/lib/hls-parser.ts');

test('real detector finds all qualities before playback and one overlay per player', (context) => {
  const dom = domHarness(context, '<video id="main" preload="none"><source src="/v.mp4?quality=720&token=a%2Bb" type="video/mp4" label="720p"><source src="/v.mp4?quality=480" type="video/mp4" label="480p"></video>');
  const detections = [];
  const overlay = new VideoOverlayManager(() => {});
  const detector = new VideoDetector({ onDetected: (media, target) => { detections.push(media); overlay.addOverlay(media, media.variants, target); } });
  detector.start();
  assert.equal(detections.length, 1);
  assert.equal(detections[0].variants.length, 2);
  assert.equal(detections[0].variants[0].url, 'https://video.example/v.mp4?quality=720&token=a%2Bb');
  assert.equal(dom.document.querySelectorAll('dlman-overlay').length, 1);
  dom.tick();
  assert.equal(detections.length, 1);
  detector.destroy(); overlay.destroy();
});

test('network hints never attach to unrelated players; confirmed sources replace a preroll safely', (context) => {
  const dom = domHarness(context, '<div id="main" class="video-js"><video src="/main.mp4"></video></div><video id="second" src="/other.mp4"></video>');
  const detections = [], removed = [];
  const detector = new VideoDetector({ onDetected: (media) => detections.push(media), onRemoved: (id) => removed.push(id) });
  detector.start();
  detector.injectStreamUrl('https://cdn.example/unrelated.m3u8', 'hls');
  dom.tick();
  assert.equal(detections.length, 2);
  const root = dom.document.querySelector('#main'), video = root.querySelector('video');
  root.classList.add('vjs-ad-playing');
  video.src = video.currentSrc = 'https://cdn.example/preroll/ad.mp4';
  dom.tick();
  assert.equal(detections.length, 2);
  assert.equal([...detector.detected.values()][0].master_url, 'https://video.example/main.mp4');
  root.classList.remove('vjs-ad-playing');
  video.src = video.currentSrc = 'https://cdn.example/new.mp4?token=next';
  dom.tick();
  assert.equal(detections.length, 3);
  assert.equal(detections[2].id, detections[0].id);
  assert.equal(detections[2].master_url, video.src);
  video.remove(); root.remove(); dom.tick();
  assert.equal(removed.length, 1);
  detector.destroy();
});

test('right-click lookup follows the frontmost player and cannot fall back across an unresolved player', (context) => {
  const dom = domHarness(context, '<video id="back" src="/back.mp4"></video><video id="front" src="/front.mp4"></video>');
  const back = dom.document.querySelector('#back'), front = dom.document.querySelector('#front');
  dom.document.elementsFromPoint = () => [front, back];
  const detector = new VideoDetector({ onDetected: () => {} });
  detector.start();
  assert.equal(detector.getMediaNearPoint(100, 100).master_url, 'https://video.example/front.mp4');
  front.src = front.currentSrc = '';
  dom.tick();
  assert.equal(detector.getMediaNearPoint(100, 100), null);
  assert.equal(detector.getAnyDetectedMedia(), null);
  detector.destroy();
});

test('visible unresolved players request extraction once; navigation and disable reject stale results', (context) => {
  const dom = domHarness(context, '<div class="jwplayer" id="main"></div>');
  const detections = [];
  const detector = new VideoDetector({ onDetected: (media) => detections.push(media) });
  detector.start();
  const original = detections[0];
  assert.equal(original.discovery_status, 'detecting');
  dom.tick(); assert.equal(detections.length, 1);
  dom.location.pathname = '/next'; dom.tick();
  assert.equal(detector.resolvePlayer(original, { ...original, discovery_status: 'ready' }), false);
  const current = detections.at(-1);
  detector.setEnabled(false);
  assert.equal(detector.resolvePlayer(current, { ...current, discovery_status: 'ready' }), false);
  detector.destroy();
});

test('replacing the video inside a retained player container invalidates the old inspection', (context) => {
  const dom = domHarness(context, '<div class="jwplayer" id="main"><video></video></div>');
  const detections = [];
  const detector = new VideoDetector({ onDetected: (media) => detections.push(media) });
  detector.start();
  const original = detections[0];
  dom.document.querySelector('#main').innerHTML = '<video></video>';
  dom.configure(); dom.tick();
  assert.equal(detections.length, 2);
  assert.notEqual(detections[1].id, original.id);
  assert.equal(detector.resolvePlayer(original, { ...original, discovery_status: 'ready' }), false);
  detector.destroy();
});

test('open shadow roots retain source ownership; multiple unresolved players stay ambiguous', (context) => {
  const dom = domHarness(context, '<div id="host"></div>');
  const root = dom.document.querySelector('#host').attachShadow({ mode: 'open' });
  root.innerHTML = '<video src="https://cdn.example/shadow.mp4"></video>';
  dom.configure(root);
  const detections = [];
  const detector = new VideoDetector({ onDetected: (media) => detections.push(media) });
  detector.start();
  assert.equal(detections[0].master_url, 'https://cdn.example/shadow.mp4');
  detector.destroy();
  dom.document.body.innerHTML = '<video></video><video></video>';
  const ambiguous = new VideoDetector({ onDetected: () => assert.fail('Ambiguous extraction attached to a player') });
  ambiguous.start(); ambiguous.destroy();
});

test('blob association requires explicit MSE evidence and cannot attach to a second player', (context) => {
  const dom = domHarness(context, '<video id="first"></video><video id="second"></video>');
  const first = dom.document.querySelector('#first'), second = dom.document.querySelector('#second');
  first.src = first.currentSrc = 'blob:https://video.example/first';
  second.src = second.currentSrc = 'blob:https://video.example/second';
  dom.document.elementsFromPoint = () => [first, second];
  const detections = [];
  const detector = new VideoDetector({ onDetected: (media) => detections.push(media) });
  detector.start();
  detector.injectStreamUrl('https://cdn.example/main.m3u8', 'hls', first.currentSrc);
  dom.tick();
  assert.equal(detections.length, 1);
  assert.equal(detections[0].master_url, 'https://cdn.example/main.m3u8');
  assert.equal(detector.getMediaNearPoint(100, 100)?.master_url, 'https://cdn.example/main.m3u8');
  detector.destroy();
});

test('fixed overlay follows CSS visibility changes without replacing qualities or closing an unchanged picker', (context) => {
  const dom = domHarness(context, '<video src="/main.mp4"></video>');
  const video = dom.document.querySelector('video');
  const overlay = new VideoOverlayManager(() => {});
  const detector = new VideoDetector({
    onDetected: (media, target) => overlay.addOverlay(media, media.variants, target),
    onVisibility: (id, visible) => overlay.setVisibility(id, visible),
  });
  detector.start();
  const host = dom.document.querySelector('dlman-overlay');
  assert.equal(host.style.display, 'block');
  globalThis.getComputedStyle = (element) => ({ display: element.style.display || 'block', visibility: 'visible', opacity: '1' });
  video.style.display = 'none';
  dom.tick();
  assert.equal(host.style.display, 'none');
  video.style.display = 'block';
  dom.tick();
  assert.equal(host.style.display, 'block');
  assert.equal(dom.document.querySelectorAll('dlman-overlay').length, 1);
  detector.destroy(); overlay.destroy();
});

test('HLS quality handoff retains master audio groups and selected resolution/frame rate', async (context) => {
  const dom = domHarness(context, '<video src="https://cdn.example/master.m3u8"></video>');
  const roots = [];
  const originalAttach = dom.window.HTMLElement.prototype.attachShadow;
  context.mock.method(dom.window.HTMLElement.prototype, 'attachShadow', function (options) {
    const root = originalAttach.call(this, options); roots.push(root); return root;
  });
  const master = 'https://cdn.example/master.m3u8';
  const variants = parseMasterPlaylist('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",URI="audio.m3u8"\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080,FRAME-RATE=60,AUDIO="audio"\n1080.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720,FRAME-RATE=30,AUDIO="audio"\n720.m3u8', master);
  const requests = [];
  const overlay = new VideoOverlayManager((request) => requests.push(request));
  overlay.addOverlay({ id: 'hls', page_url: location.href, master_url: master, protocol: 'hls', variants },
    variants, dom.document.querySelector('video'));
  roots[0].querySelector('.pill').click();
  roots[0].querySelectorAll('.dd-item')[2].click();
  await Promise.resolve();
  assert.equal(requests[0].media.master_url, master);
  assert.deepEqual(requests[0].media_quality, { height: 720, fps: 30, label: '720p' });
  overlay.destroy();
});

test('automatic discovery waits for intersection confirmation including ancestor-frame clipping', (context) => {
  const dom = domHarness(context, '<div id="player" class="jwplayer"></div>');
  let callback, target;
  globalThis.IntersectionObserver = class {
    constructor(handler) { callback = handler; }
    observe(element) { target = element; }
    unobserve() {} disconnect() {}
  };
  const detections = [];
  const detector = new VideoDetector({ onDetected: (media) => detections.push(media) });
  detector.start();
  assert.equal(detections.length, 0);
  callback([{ target, isIntersecting: false }]); dom.tick();
  assert.equal(detections.length, 0);
  callback([{ target, isIntersecting: true }]); dom.tick();
  assert.equal(detections.length, 1);
  callback([{ target, isIntersecting: false }]); dom.tick();
  assert.equal(detector.isVisible(detections[0].id), false);
  detector.destroy();
});

test('unresolved pill cannot download; ready qualities update the same overlay and preserve selection', async (context) => {
  const dom = domHarness(context, '<video></video>');
  const roots = [];
  const attach = dom.window.HTMLElement.prototype.attachShadow;
  context.mock.method(dom.window.HTMLElement.prototype, 'attachShadow', function (options) {
    const root = attach.call(this, options); roots.push(root); return root;
  });
  const requests = [], overlay = new VideoOverlayManager((request) => requests.push(request));
  const media = { id: 'pending', page_url: location.href, master_url: location.href, protocol: 'webpage', discovery_status: 'detecting', variants: [] };
  const video = dom.document.querySelector('video');
  overlay.addOverlay(media, [], video);
  const pill = roots[0].querySelector('.pill');
  assert.equal(pill.getAttribute('aria-disabled'), 'true');
  assert.ok(pill.textContent.includes('Detecting video…'));
  pill.click(); assert.equal(requests.length, 0);
  const selection = { identity: 'video', format_id: '720', container: 'mp4', quality: '720p', output_mode: 'video' };
  const variants = [
    { url: location.href, label: '1080p', web_media_selection: { ...selection, format_id: '1080', quality: '1080p' } },
    { url: location.href, label: '720p', web_media_selection: selection },
  ];
  overlay.addOverlay({ ...media, discovery_status: 'ready', inspection_id: 'native', variants }, variants, video);
  assert.equal(pill.getAttribute('aria-disabled'), 'false');
  assert.ok(pill.querySelector('.chev'));
  assert.equal(dom.document.querySelectorAll('dlman-overlay').length, 1);
  pill.click(); roots[0].querySelectorAll('.dd-item')[2].click();
  await Promise.resolve();
  assert.deepEqual(requests[0].web_media_selection, selection);
  assert.equal(requests[0].inspection_id, 'native');
  overlay.destroy();
});
