import test from 'node:test';
import assert from 'node:assert/strict';
// The production store persists through the browser storage boundary.
const stored = new Map();
globalThis.localStorage = {
  getItem: key => stored.get(key) ?? null,
  setItem: (key, value) => stored.set(key, value),
  removeItem: key => stored.delete(key),
};
const { useDownloadStore } = await import('../src/stores/downloads.ts');

test('segment progress finds a stable ID after ranges are reordered and split', () => {
  const segments = [
    { index: 0, start: 0, end: 99, downloaded: 100, complete: true },
    { index: 8, start: 100, end: 149, downloaded: 0, complete: false },
    { index: 2, start: 150, end: 199, downloaded: 10, complete: false },
  ];
  useDownloadStore.setState({ downloads: new Map([['job', { id: 'job', segments }]]) });
  useDownloadStore.getState().updateSegmentProgress('job', 8, 25);
  const updated = useDownloadStore.getState().downloads.get('job').segments;
  assert.equal(updated[1].downloaded, 25);
  assert.equal(updated[2].downloaded, 10);
  assert.equal(segments[1].downloaded, 0);
  useDownloadStore.getState().updateSegmentProgress('job', 99, 20);
  assert.equal(useDownloadStore.getState().downloads.get('job').segments.length, 3);
});

test('reordered segment updates clamp progress and mark exact completion', () => {
  useDownloadStore.setState({ downloads: new Map([['job', { id: 'job', segments: [
    { index: 6, start: 50, end: 99, downloaded: 0, complete: false },
  ] }]]) });
  useDownloadStore.getState().updateSegmentProgress('job', 6, 100);
  const segment = useDownloadStore.getState().downloads.get('job').segments[0];
  assert.equal(segment.downloaded, 50);
  assert.equal(segment.complete, true);
});
