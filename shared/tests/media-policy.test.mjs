import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyMediaCandidate } from '../media-policy.mjs';

test('excludes ad hosts, explicit ad state and transport resources across protocols', () => {
  for (const url of [
    'https://ads.doubleclick.net/main.mp4',
    'https://cdn.example/ads/master.m3u8',
    'https://cdn.example/vast.xml',
    'https://cdn.example/chunks/part.mp4',
    'https://cdn.example/segment.m4s',
    'https://cdn.example/init.mp4',
    'https://cdn.example/subtitles.vtt',
  ]) assert.equal(classifyMediaCandidate({ url, mime: 'video/mp4' }), null, url);
  assert.equal(classifyMediaCandidate({ url: 'https://cdn.example/main.mp4', isAd: true }), null);
  assert.equal(classifyMediaCandidate({ url: 'https://cdn.example/main.mp4', mime: 'video/mp2t' }), null);
});

test('uses host boundaries and retains signed URLs, trailing slashes and real short clips', () => {
  const url = 'https://notdoubleclick.net/get_file/analytics-lesson.mp4/?token=a%2Bb&quality=720';
  assert.deepEqual(classifyMediaCandidate({ url, duration: 3, loop: true, muted: true }), { url, protocol: 'direct' });
  assert.equal(classifyMediaCandidate({ url: 'https://cdn.example/master.m3u8/' }).protocol, 'hls');
  assert.equal(classifyMediaCandidate({ url: 'https://cdn.example/manifest', mime: 'application/dash+xml' }).protocol, 'dash');
  for (const url of ['blob:https://video.example/1', 'http://127.0.0.1/a.mp4', 'https://user:secret@cdn.example/a.mp4']) {
    assert.equal(classifyMediaCandidate({ url }), null);
  }
});
