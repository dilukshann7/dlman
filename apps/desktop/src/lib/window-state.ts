import { getCurrentWindow, LogicalSize, type Window } from '@tauri-apps/api/window';

const STORAGE_KEY = 'dlman-window-state';

interface WindowState {
  width: number;
  height: number;
  maximized: boolean;
}

type ManagedWindow = Pick<Window,
  'innerSize' | 'scaleFactor' | 'isMinimized' | 'isMaximized' | 'isFullscreen' |
  'setSize' | 'maximize' | 'center' | 'onResized'>;
type StateStorage = Pick<Storage, 'getItem' | 'setItem'>;

function validSize(width: unknown, height: unknown): width is number {
  return typeof width === 'number' && Number.isFinite(width) && width >= 800 &&
    typeof height === 'number' && Number.isFinite(height) && height >= 600;
}

function readState(storage: StateStorage): WindowState | undefined {
  try {
    const state: unknown = JSON.parse(storage.getItem(STORAGE_KEY) ?? 'null');
    if (state && typeof state === 'object' &&
      'width' in state && 'height' in state && 'maximized' in state &&
      validSize(state.width, state.height) && typeof state.maximized === 'boolean') {
      return { width: state.width, height: state.height as number, maximized: state.maximized };
    }
  } catch {
    // Missing, corrupt or inaccessible storage must not prevent startup.
  }
}

/** Persist normal client dimensions in logical pixels across app launches. */
export async function rememberWindowState(
  appWindow: ManagedWindow = getCurrentWindow(),
  storage: StateStorage = localStorage,
): Promise<() => void> {
  let normalState = readState(storage);
  if (normalState) {
    try {
      await appWindow.setSize(new LogicalSize(normalState.width, normalState.height));
      await appWindow.center();
      if (normalState.maximized) await appWindow.maximize();
    } catch (error) {
      console.warn('Failed to restore window state:', error);
    }
  }

  let disposed = false;
  let revision = 0;
  const saveState = async () => {
    const currentRevision = ++revision;
    try {
      const [minimized, maximized, fullscreen] = await Promise.all([
        appWindow.isMinimized(), appWindow.isMaximized(), appWindow.isFullscreen(),
      ]);
      if (disposed || currentRevision !== revision || minimized || fullscreen) return;

      let nextState = normalState && { ...normalState, maximized };
      if (!maximized) {
        const [size, scale] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()]);
        if (!Number.isFinite(scale) || scale <= 0) return;
        const width = Math.round(size.width / scale);
        const height = Math.round(size.height / scale);
        if (!validSize(width, height)) return;
        nextState = { width, height, maximized: false };
      }
      // Native queries can finish out of order during rapid resizing or teardown.
      if (disposed || currentRevision !== revision || !nextState) return;
      storage.setItem(STORAGE_KEY, JSON.stringify(nextState));
      normalState = nextState;
    } catch (error) {
      console.warn('Failed to save window state:', error);
    }
  };

  const unlisten = await appWindow.onResized(saveState);
  await saveState();
  return () => {
    disposed = true;
    revision++;
    unlisten();
  };
}
