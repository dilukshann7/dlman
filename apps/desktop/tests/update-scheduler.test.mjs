import test from 'node:test';
import assert from 'node:assert/strict';
import { startUpdateSchedule, UPDATE_INTERVAL_MS } from '../src/lib/update-scheduler.ts';

function fixture() {
  let next = 0;
  const timers = new Map();
  const clock = {
    setTimeout: (fn, delay) => { timers.set(++next, { fn, delay }); return next; },
    clearTimeout: id => timers.delete(id),
  };
  let checks = 0;
  return { clock, timers, check: async () => { checks++; }, count: () => checks };
}

test('opt-out or unloaded settings schedules no checks', () => {
  const f = fixture();
  startUpdateSchedule({ loaded: false, enabled: true, check: f.check, clock: f.clock });
  startUpdateSchedule({ loaded: true, enabled: false, check: f.check, clock: f.clock });
  assert.equal(f.timers.size, 0);
});

test('StrictMode cleanup then replay retains one startup check and six-hour schedule', async () => {
  const f = fixture();
  const options = { loaded: true, enabled: true, check: f.check, clock: f.clock };
  const cleanup = startUpdateSchedule(options);
  cleanup();
  const stop = startUpdateSchedule(options);
  assert.equal(f.timers.size, 1);
  const [id, timer] = [...f.timers.entries()][0];
  assert.equal(timer.delay, 3000);
  f.timers.delete(id);
  await timer.fn();
  assert.equal(f.count(), 1);
  assert.equal([...f.timers.values()][0].delay, UPDATE_INTERVAL_MS);
  stop();
  assert.equal(f.timers.size, 0);
});

test('cleanup during an in-flight check prevents rescheduling', async () => {
  const f = fixture();
  let finish;
  const stop = startUpdateSchedule({
    loaded: true, enabled: true, clock: f.clock,
    check: () => new Promise(resolve => { finish = resolve; }),
  });
  const [id, timer] = [...f.timers.entries()][0];
  f.timers.delete(id);
  const pending = timer.fn();
  stop();
  finish();
  await pending;
  assert.equal(f.timers.size, 0);
});
