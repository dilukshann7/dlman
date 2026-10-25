import test from 'node:test';
import assert from 'node:assert/strict';
import { AutomaticInspectionQueue } from '../media-discovery.mjs';

test('short RPC queue starts one native inspection, polls existing work and then releases the next job', async () => {
  let now = 0, starts = 0, polls = 0;
  const queue = new AutomaticInspectionQueue({ now: () => now, poll: async (id) => {
    polls++;
    return { id, status: 'ready', preview: { variants: [] } };
  } });
  const a = { key: 'a', isCurrent: () => true, start: async () => ({ id: 'native-' + ++starts, status: 'pending' }) };
  const b = { ...a, key: 'b' };
  assert.equal((await queue.request(a)).id, 'native-1');
  assert.equal((await queue.request(b)).status, 'pending');
  assert.equal(starts, 1);
  now = 1000;
  assert.equal((await queue.request(a)).status, 'ready');
  assert.equal(starts, 2);
  assert.equal(polls, 1);
  queue.clear();
  assert.equal((await queue.request(b)).id, 'native-2');
  assert.equal(starts, 2);
});

test('short RPC queue caches failed/ready results and rejects stale documents', async () => {
  let now = 0, starts = 0, current = true;
  const queue = new AutomaticInspectionQueue({ now: () => now, poll: async () => assert.fail('No native job is pending') });
  const job = { key: 'a', isCurrent: () => current, start: async () => {
    starts++; return { id: 'id', status: starts < 2 ? 'failed' : 'ready' };
  } };
  await queue.request(job); await queue.request(job); assert.equal(starts, 1);
  await queue.request(job, true); assert.equal(starts, 2);
  now = 120001; await queue.request(job); assert.equal(starts, 3);
  current = false;
  await assert.rejects(queue.request(job), /changed/);
});

test('native busy after a worker restart stays queued and retries without failing the video', async () => {
  let starts = 0;
  const queue = new AutomaticInspectionQueue({ poll: async () => assert.fail('Queued work has no native ID yet') });
  const task = { key: 'a', isCurrent: () => true, start: async () => {
    starts++; return starts === 1 ? { id: 'queued', status: 'pending' } : { id: 'native', status: 'ready' };
  } };
  assert.equal((await queue.request(task)).id, 'queued');
  assert.equal((await queue.request(task)).status, 'ready');
  assert.equal(starts, 2);
});

test('queued player losing eligibility never starts when another native task completes', async () => {
  let now = 0, eligible = true;
  const starts = [];
  const queue = new AutomaticInspectionQueue({ now: () => now, poll: async (id) => ({ id, status: 'ready' }) });
  const task = (key, isCurrent = () => true) => ({ key, isCurrent, start: async () => {
    starts.push(key); return { id: key, status: 'pending' };
  } });
  await queue.request(task('first'));
  await queue.request(task('removed', () => eligible));
  eligible = false; now = 1000;
  await queue.request(task('third'));
  assert.deepEqual(starts, ['first', 'third']);
});
