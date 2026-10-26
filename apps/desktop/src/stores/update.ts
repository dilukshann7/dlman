/** Shared update lifecycle. Only dismissal is persisted; native payloads are session-local. */
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface UpdateInfo {
  hasUpdate: boolean;
  latestVersion: string | null;
  currentVersion: string;
  releaseUrl: string;
  releaseNotes?: string | null;
  publishedAt?: string | null;
  supported: boolean;
  unsupportedReason: 'package-manager' | 'unsupported-platform' | 'development' | null;
}
type UpdateStatus = 'idle' | 'checking' | 'up-to-date' | 'available' | 'downloading' | 'ready' | 'installing' | 'error';
interface UpdateStore {
  updateInfo: UpdateInfo | null;
  dismissedVersion: string | null;
  status: UpdateStatus;
  error: string | null;
  failedAction: 'check' | 'download' | 'install' | null;
  downloaded: number;
  total: number | null;
  canInstall: boolean;
  checkForUpdates: () => Promise<void>;
  downloadUpdate: () => Promise<void>;
  installUpdate: () => Promise<void>;
  refreshReadiness: () => Promise<void>;
  dismissUpdate: () => void;
  markAboutSeen: () => void;
  shouldShowNotification: () => boolean;
}
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
let checkPromise: Promise<void> | null = null;
let downloadPromise: Promise<void> | null = null;
let installPromise: Promise<void> | null = null;

export const useUpdateStore = create<UpdateStore>()(
  persist((set, get) => ({
    updateInfo: null, dismissedVersion: null, status: 'idle', error: null, failedAction: null,
    downloaded: 0, total: null, canInstall: false,
    checkForUpdates: () => {
      if (checkPromise) return checkPromise;
      if (downloadPromise || installPromise || get().status === 'ready') return Promise.resolve();
      set({ status: 'checking', error: null, failedAction: null });
      checkPromise = (async () => {
        try {
          const updateInfo = await invoke<UpdateInfo>('check_app_update');
          set({ updateInfo, status: updateInfo.hasUpdate ? 'available' : 'up-to-date' });
        } catch (error) {
          set({ status: 'error', error: message(error), failedAction: 'check' });
        }
      })().finally(() => { checkPromise = null; });
      return checkPromise;
    },
    downloadUpdate: () => {
      if (downloadPromise) return downloadPromise;
      if (!get().updateInfo?.supported || !get().updateInfo?.hasUpdate ||
          !['available', 'error'].includes(get().status) || checkPromise || installPromise) return Promise.resolve();
      set({ status: 'downloading', error: null, failedAction: null, downloaded: 0, total: null });
      downloadPromise = (async () => {
        let unlisten: (() => void) | undefined;
        try {
          // Register before starting the native request; fast downloads must not lose events.
          unlisten = await listen<{ downloaded: number; total: number | null }>('app-update-progress', event => {
            if (get().status === 'downloading') set(event.payload);
          });
          await invoke('download_app_update');
          set({ status: 'ready', canInstall: false });
          await get().refreshReadiness();
        } catch (error) {
          set({ status: 'error', error: message(error), failedAction: 'download' });
        } finally { unlisten?.(); }
      })().finally(() => { downloadPromise = null; });
      return downloadPromise;
    },
    refreshReadiness: async () => {
      if (get().status !== 'ready') return;
      try {
        const canInstall = await invoke<boolean>('can_install_app_update');
        if (get().status === 'ready') set({ canInstall });
      } catch {
        if (get().status === 'ready') set({ canInstall: false });
      }
    },
    installUpdate: () => {
      if (installPromise) return installPromise;
      if (get().status !== 'ready') return Promise.resolve();
      installPromise = (async () => {
        await get().refreshReadiness();
        if (!get().canInstall) return;
        set({ status: 'installing', error: null, failedAction: null });
        try {
          // Native code takes the exclusive guard, closing the UI-to-command race.
          await invoke('install_app_update');
        } catch (error) {
          set({ status: 'ready', error: message(error), failedAction: 'install', canInstall: false });
          await get().refreshReadiness();
        }
      })().finally(() => { installPromise = null; });
      return installPromise;
    },
    dismissUpdate: () => set({ dismissedVersion: get().updateInfo?.latestVersion ?? get().dismissedVersion }),
    markAboutSeen: () => get().dismissUpdate(),
    shouldShowNotification: () => {
      const { updateInfo, dismissedVersion, status } = get();
      return status === 'downloading' || status === 'ready' || status === 'installing' ||
        Boolean(updateInfo?.hasUpdate && updateInfo.latestVersion !== dismissedVersion);
    },
  }), {
    name: 'dlman-update-store',
    partialize: state => ({ dismissedVersion: state.dismissedVersion }),
  }),
);
