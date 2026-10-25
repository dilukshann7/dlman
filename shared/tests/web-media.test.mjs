import test from 'node:test';
import assert from 'node:assert/strict';
import { isWebMediaUrl, sortWebMediaVariants, webMediaFilename, webMediaSelection, webMediaSelectionForQuality, validateWebMediaSelection, awaitWebMediaInspection, webMediaCaptureBridge } from '../web-media.mjs';

const variant = (id, quality, fps = 30, container = 'mp4') => ({ id, quality, fps, container, audio: 'AAC', output_mode: 'video' });

test('HLS quality matches a complete parent format and never silently switches resolution or frame rate', () => {
  const preview = { identity: 'master', variants: [variant('1080+audio', '1080p', 60), variant('720+audio', '720p', 30)] };
  assert.equal(webMediaSelectionForQuality(preview, { height: 720, fps: 30, label: '720p' }).format_id, '720+audio');
  assert.throws(() => webMediaSelectionForQuality(preview, { height: 720, fps: 60, label: '720p' }), /unavailable/);
  assert.throws(() => webMediaSelectionForQuality(preview, { height: 480, label: '480p' }), /unavailable/);
});

test('generic media accepts HTTP(S), excluding YouTube and credential URLs', () => {
  assert.equal(isWebMediaUrl('https://example.com/watch/123'), true);
  for (const url of ['https://youtube.com/watch?v=abcdefghijk', 'https://youtu.be/abcdefghijk', 'file:///video.mp4', 'https://user:password@example.com/video']) assert.equal(isWebMediaUrl(url), false);
});

test('quality sorts by resolution then fps then MP4; no quality is removed', () => {
  const variants = [variant('720', '720p', 60), variant('webm', '1080p', 60, 'webm'), variant('mp4', '1080p', 60), variant('30', '1080p', 30)];
  assert.deepEqual(sortWebMediaVariants(variants).map((v) => v.id), ['mp4', 'webm', '30', '720']);
  assert.equal(variants[0].id, '720');
  assert.equal(sortWebMediaVariants([variants[0]]).length, 1);
  assert.equal(sortWebMediaVariants([variant('720', '720p'), variant('1080', '1080p60')])[0].id, '1080');
});

test('filenames sanitize title and preserve edited basename when container changes', () => {
  assert.equal(webMediaFilename('A: video / title', 'mp4'), 'A_ video _ title.mp4');
  assert.equal(webMediaFilename('Other title', 'mkv', 'My edited.name.mp4'), 'My edited.name.mkv');
  assert.equal(webMediaFilename('', 'mp4'), 'Video.mp4');
});

test('selection uses stable ID and rejects changed identity, quality or container', () => {
  const preview = { identity: 'one', variants: [variant('stable', '1080p')] };
  const selection = webMediaSelection(preview, preview.variants[0]);
  assert.equal(validateWebMediaSelection(preview, selection), true);
  assert.equal(validateWebMediaSelection({ ...preview, identity: 'other' }, selection), false);
  assert.equal(validateWebMediaSelection({ ...preview, variants: [variant('different', '1080p')] }, selection), false);
  assert.equal(validateWebMediaSelection({ ...preview, variants: [variant('stable', '720p')] }, selection), false);
});

test('inspection polls one ID, accepts file classification and rejects failed extraction', async () => {
  const ids = [];
  const result = await awaitWebMediaInspection(async () => ({ id: 'one', status: 'pending' }), async (id) => { ids.push(id); return { id, status: 'file' }; }, { delay: async () => {} });
  assert.equal(result.status, 'file');
  assert.deepEqual(ids, ['one']);
  await assert.rejects(awaitWebMediaInspection(async () => ({ status: 'failed', error: 'No video found' }), async () => {}), /No video found/);
  await assert.rejects(awaitWebMediaInspection(async () => ({ status: 'ready', preview: { variants: [] } }), async () => {}), /No downloadable video/);
});

test('stale completed inspection and pending timeouts never become handoffs', async () => {
  await assert.rejects(awaitWebMediaInspection(async () => ({ status: 'file' }), async () => {}, { isCurrent: () => false }), /URL changed/);
  let now = 0;
  await assert.rejects(awaitWebMediaInspection(async () => ({ id: 'same', status: 'pending' }), async (id) => ({ id, status: 'pending' }), { now: () => now++, timeout: 1, delay: async () => {} }), /timed out/);
});

test('browser capture opens only a loopback bridge with matching ticket', () => {
  const id = 'abcd-1234';
  assert.equal(webMediaCaptureBridge({ id, bridge_url: `http://127.0.0.1:5000/web-media-capture#${id}` }), `http://127.0.0.1:5000/web-media-capture#${id}`);
  for (const bridge_url of [`https://example.com/web-media-capture#${id}`, `http://127.0.0.1:5000/web-media-capture#wrong`, `http://127.0.0.1:5000/other#${id}`]) assert.throws(() => webMediaCaptureBridge({ id, bridge_url }), /capture bridge/);
});
