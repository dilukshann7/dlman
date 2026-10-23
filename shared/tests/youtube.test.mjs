import test from 'node:test';
import assert from 'node:assert/strict';
import { canonicalYoutubeUrl, isYoutubeHost, videoChoices, awaitYoutubeInspection, createYoutubeDialogQueue } from '../youtube.mjs';

test('watch, Shorts, mobile and short links share one canonical identity', () => {
  for (const url of [
    'https://www.youtube.com/watch?v=abcdefghijk&list=ignored&t=30',
    'https://m.youtube.com/shorts/abcdefghijk',
    'https://youtu.be/abcdefghijk?si=tracking',
  ]) assert.equal(canonicalYoutubeUrl(url), 'https://www.youtube.com/watch?v=abcdefghijk');
});

test('rejects non-video URLs, impersonators, credentials and malformed IDs', () => {
  for (const url of [
    'https://evil.youtube.com/watch?v=abcdefghijk',
    'https://youtube.com/playlist?list=x', 'https://youtube.com/live/abcdefghijk',
    'https://youtube.com/watch?v=bad', 'https://user@youtube.com/watch?v=abcdefghijk',
    'file:///watch?v=abcdefghijk', 'https://youtube.com:123/watch?v=abcdefghijk',
  ]) assert.equal(canonicalYoutubeUrl(url), null);
  assert.equal(isYoutubeHost('https://youtube.com/playlist?list=x'), true);
  assert.equal(isYoutubeHost('https://youtube.com.evil.test/watch?v=abcdefghijk'), false);
});

test('Download All excludes the explicit MP3 conversion choice', () => {
  const video = { id: '137+140', output_mode: 'video' };
  assert.deepEqual(videoChoices([video, { id: '251', output_mode: 'mp3' }]), [video]);
});

test('cold inspections poll using the same backend ID until ready', async () => {
  const calls = [];
  const preview = { url: 'canonical', variants: [] };
  const result = await awaitYoutubeInspection(async () => ({ id: 'one', status: 'pending' }), async (id) => {
    calls.push(id);
    return calls.length === 1 ? { id, status: 'pending' } : { id, status: 'ready', preview };
  }, { delay: async () => {} });
  assert.deepEqual(calls, ['one', 'one']);
  assert.equal(result.preview, preview);
});

test('navigation invalidates even a completed response from the previous video', async () => {
  await assert.rejects(awaitYoutubeInspection(async () => ({ status: 'ready', preview: {} }), async () => {},
    { isCurrent: () => false }), /Video changed/);
});

test('pending inspections time out without starting another extraction', async () => {
  let clock = 0;
  await assert.rejects(awaitYoutubeInspection(async () => ({ id: 'same', status: 'pending' }),
    async (id) => ({ id, status: 'pending' }),
    { now: () => clock++, timeout: 1, delay: async () => {} }), /timed out/);
});

test('missing tools and backend failures never become successful handoffs', async () => {
  await assert.rejects(awaitYoutubeInspection(async () => ({ status: 'failed', error: 'Tools missing' }), async () => {}), /Tools missing/);
});

test('Download All preserves every quality instead of replacing an open dialog', () => {
  let isOpen = false;
  const shown = [], scheduled = [];
  const queue = createYoutubeDialogQueue((payload) => { shown.push(payload); isOpen = true; }, () => isOpen, (fn) => scheduled.push(fn));
  queue.offer('1080p'); queue.offer('720p'); queue.offer('480p');
  assert.deepEqual(shown, ['1080p']);
  isOpen = false; queue.closed();
  queue.offer('360p');
  scheduled.shift()();
  assert.deepEqual(shown, ['1080p', '720p']);
  isOpen = false; queue.closed(); scheduled.shift()();
  assert.deepEqual(shown, ['1080p', '720p', '480p']);
  queue.destroy(); isOpen = false; queue.closed();
  assert.equal(scheduled.length, 0);
});
