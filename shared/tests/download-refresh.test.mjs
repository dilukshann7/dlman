import test from 'node:test';
import assert from 'node:assert/strict';
import { canRefreshDownload, refreshSessionExpired, displayRefreshUrl } from '../download-refresh.mjs';

const job = { url: 'https://files.example/file.zip', status: 'paused' };
test('only paused and failed ordinary HTTP downloads can refresh', () => {
  assert.equal(canRefreshDownload(job), true);
  assert.equal(canRefreshDownload({ ...job, status: 'failed' }), true);
  for (const status of ['pending', 'downloading', 'queued', 'completed', 'cancelled', 'deleted']) {
    assert.equal(canRefreshDownload({ ...job, status }), false);
  }
  for (const url of ['file:///tmp/file', 'https://youtube.com/watch?v=123', 'https://youtu.be/123', 'https://cdn.example/video.m3u8?token=1', 'https://cdn.example/video.mpd', 'invalid']) {
    assert.equal(canRefreshDownload({ ...job, url }), false);
  }
  assert.equal(canRefreshDownload({ ...job, youtube: {} }), false);
});
test('session expires at its deadline', () => {
  assert.equal(refreshSessionExpired({ expires_at: 1000 }, 999), false);
  assert.equal(refreshSessionExpired({ expires_at: 1000 }, 1000), true);
});
test('candidate display never reveals signed queries or URL credentials', () => {
  assert.equal(displayRefreshUrl('https://user:secret@cdn.example/file?token=secret#fragment'), 'https://cdn.example/file');
  assert.equal(displayRefreshUrl('invalid'), '');
});
