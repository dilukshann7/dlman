/** Short RPC steps survive MV3 suspension; the native service owns the running extraction. */
export class AutomaticInspectionQueue {
  constructor({ poll, now = Date.now }) {
    this.poll = poll; this.now = now; this.jobs = new Map(); this.active = undefined;
    this.tail = Promise.resolve();
  }
  request(task, retry = false) {
    const result = this.tail.then(() => this.step(task, retry));
    this.tail = result.catch(() => {});
    return result;
  }
  async step(task, retry) {
    if (!task.isCurrent()) throw new Error('Video page changed.');
    let job = this.jobs.get(task.key);
    if (job && job !== this.active && (retry || !job.failed && job.state.status !== 'pending' && this.now() - job.created >= 120000)) {
      this.jobs.delete(task.key); job = undefined;
    }
    if (!job) {
      if (this.jobs.size >= 128) {
        for (const [key, old] of this.jobs) if (old !== this.active && old.state.status !== 'pending') this.jobs.delete(key);
        if (this.jobs.size >= 128) throw new Error('Too many video inspections. Retry shortly.');
      }
      job = { task, state: { id: 'queued', status: 'pending' }, created: this.now(), failed: false };
      this.jobs.set(task.key, job);
    }
    const finish = (entry, state) => {
      entry.state = state; entry.created = this.now(); entry.failed = state.status === 'failed';
      if (state.status === 'pending' && state.id !== 'queued') { this.active = entry; entry.polled = this.now(); }
      else if (this.active === entry) this.active = undefined;
    };
    if (this.active && this.now() - this.active.polled >= 500) {
      const active = this.active;
      try { finish(active, await this.poll(active.state.id)); }
      catch { finish(active, { id: active.state.id, status: 'failed', error: 'DLMan is unavailable. Reconnect or retry.' }); }
    }
    if (!this.active) {
      for (const [key, pending] of this.jobs) {
        if (pending.state.status !== 'pending') continue;
        if (!pending.task.isCurrent()) { this.jobs.delete(key); continue; }
        try { finish(pending, await pending.task.start()); }
        catch (error) {
          if (error?.deferred) continue;
          finish(pending, { id: 'failed', status: 'failed', error: error instanceof Error ? error.message : 'Video inspection failed.' });
        }
        break;
      }
    }
    if (!task.isCurrent()) throw new Error('Video page changed.');
    return job.state;
  }
  clear() {
    // Keep polling any native task that already started before scheduling another.
    const active = this.active;
    this.jobs.clear();
    if (active) this.jobs.set(active.task.key, active);
  }
}
