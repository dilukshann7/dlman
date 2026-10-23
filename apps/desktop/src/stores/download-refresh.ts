import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import i18n from '@/i18n';
import type { Download } from '@/types';
import { canRefreshDownload, refreshSessionExpired } from '../../../../shared/download-refresh.mjs';

export interface RefreshSession {
  session_id: string;
  download_id: string;
  expires_at: number;
  candidate: { candidate_id: string; display_url: string; filename: string | null } | null;
}

interface RefreshState {
  download: Download | null;
  session: RefreshSession | null;
  busy: boolean;
  error: string | null;
  open: (download: Download) => Promise<void>;
  close: () => Promise<void>;
  offer: (session: RefreshSession) => void;
  verify: (url?: string, candidateId?: string) => Promise<void>;
  releaseCandidate: () => Promise<void>;
}

let generation = 0;
let pendingOffer: RefreshSession | null = null;
function consumeOffer(session: RefreshSession): RefreshSession {
  const offered = pendingOffer;
  pendingOffer = null;
  return offered?.session_id === session.session_id ? offered : session;
}
export const useDownloadRefresh = create<RefreshState>((set, get) => ({
  download: null, session: null, busy: false, error: null,
  open: async (download) => {
    if (get().download || !canRefreshDownload(download)) return;
    const request = ++generation;
    pendingOffer = null;
    set({ download, session: null, busy: true, error: null });
    try {
      const session = await invoke<RefreshSession>('begin_download_refresh', { id: download.id });
      if (request !== generation) {
        await invoke('cancel_download_refresh', { session_id: session.session_id });
        return;
      }
      set({ session: consumeOffer(session), busy: false });
    } catch (error) {
      if (request === generation) set({ busy: false, error: String(error) });
    }
  },
  close: async () => {
    const session = get().session;
    try {
      if (session) await invoke('cancel_download_refresh', { session_id: session.session_id });
      ++generation;
      pendingOffer = null;
      set({ download: null, session: null, busy: false, error: null });
    } catch {
      set({ error: i18n.t('refreshAddress.cancelFailed') });
    }
  },
  offer: (session) => {
    if (refreshSessionExpired(session)) return;
    if (!get().session && get().busy && get().download?.id === session.download_id) {
      pendingOffer = session;
      return;
    }
    if (session.session_id === get().session?.session_id && !refreshSessionExpired(session)) {
      set({ session });
    }
  },
  verify: async (url, candidateId) => {
    const { session, busy } = get();
    if (!session || busy || refreshSessionExpired(session)) return;
    set({ busy: true, error: null });
    try {
      const result = await invoke<{ download: Download; started: boolean; error: string | null }>('refresh_download_address', {
        session_id: session.session_id, url: url?.trim() || null, candidate_id: candidateId || null,
      });
      if (get().session?.session_id !== session.session_id) return;
      ++generation;
      set({ download: null, session: null, busy: false, error: null });
      if (result.started) toast.success(i18n.t('refreshAddress.success'));
      else toast.error(result.error || i18n.t('refreshAddress.resumeFailed'));
    } catch (error) {
      if (get().session?.session_id === session.session_id) set({ busy: false, error: String(error) });
    }
  },
  releaseCandidate: async () => {
    const { session, busy } = get();
    if (!session?.candidate || busy) return;
    set({ busy: true, error: null });
    try {
      await invoke('release_refresh_candidate', { session_id: session.session_id, candidate_id: session.candidate.candidate_id });
      if (get().session?.session_id !== session.session_id) return;
      ++generation;
      set({ download: null, session: null, busy: false, error: null });
    } catch (error) {
      if (get().session?.session_id === session.session_id) set({ busy: false, error: String(error) });
    }
  },
}));
