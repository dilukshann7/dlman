export const UPDATE_INTERVAL_MS = 6 * 60 * 60 * 1000;
interface Clock {
  setTimeout: (callback: () => void, milliseconds: number) => ReturnType<typeof setTimeout>;
  clearTimeout: (timer: ReturnType<typeof setTimeout>) => void;
}
/** Each effect owns its timers; cleanup never consumes a global once-only flag. */
export function startUpdateSchedule({ loaded, enabled, check, clock = globalThis }: {
  loaded: boolean; enabled: boolean; check: () => Promise<void>; clock?: Clock;
}): () => void {
  if (!loaded || !enabled) return () => {};
  let stopped = false;
  let timer: ReturnType<typeof setTimeout>;
  const run = async () => {
    try { await check(); }
    finally {
      if (!stopped) timer = clock.setTimeout(run, UPDATE_INTERVAL_MS);
    }
  };
  timer = clock.setTimeout(run, 3000);
  return () => { stopped = true; clock.clearTimeout(timer); };
}
