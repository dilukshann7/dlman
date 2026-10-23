import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-shell';
import { canonicalYoutubeUrl } from '../../../../shared/youtube.mjs';

interface Verification { id: string; url: string }

export function useYoutubeVerification(url: string, active: boolean, onVerified: () => void) {
  const [pending, setPending] = useState<Verification>();
  const [error, setError] = useState<string>();
  const [opening, setOpening] = useState(false);
  const generation = useRef(0);
  const mounted = useRef(true);
  const current = useRef({ url, active });
  current.current = { url, active };
  const completedCallback = useRef(onVerified);
  completedCallback.current = onVerified;

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; generation.current++; };
  }, []);

  useEffect(() => {
    generation.current++;
    setPending(undefined);
    setError(undefined);
    setOpening(false);
  }, [url, active]);

  useEffect(() => {
    if (!pending) return;
    let stopped = false;
    let completed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const ready = await invoke<boolean>('get_youtube_verification', { id: pending.id });
        if (stopped || !current.current.active || canonicalYoutubeUrl(current.current.url) !== canonicalYoutubeUrl(pending.url)) return;
        if (ready) {
          completed = true;
          setPending(undefined);
          completedCallback.current();
        } else timer = setTimeout(() => { void poll(); }, 1000);
      } catch (failure) {
        if (!stopped) { setError(String(failure)); setPending(undefined); }
      }
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
      if (!completed) void invoke('cancel_youtube_verification', { id: pending.id }).catch(() => {});
    };
  }, [pending]);

  const verify = useCallback(async () => {
    const canonical = canonicalYoutubeUrl(url);
    if (!canonical || !active || opening || pending) return;
    const attempt = ++generation.current;
    setOpening(true);
    setError(undefined);
    let request: Verification | undefined;
    try {
      request = await invoke<Verification>('begin_youtube_verification', { url: canonical });
      if (!mounted.current || attempt !== generation.current || !current.current.active || current.current.url !== url) {
        await invoke('cancel_youtube_verification', { id: request.id });
        return;
      }
      await open(request.url);
      if (!mounted.current || attempt !== generation.current || !current.current.active || current.current.url !== url) {
        await invoke('cancel_youtube_verification', { id: request.id });
        return;
      }
      setPending(request);
    } catch (failure) {
      if (request) void invoke('cancel_youtube_verification', { id: request.id }).catch(() => {});
      if (attempt === generation.current) setError(String(failure));
    } finally { if (attempt === generation.current) setOpening(false); }
  }, [url, active, opening, pending]);

  const cancel = useCallback(() => { generation.current++; setOpening(false); setPending(undefined); }, []);
  return { verify, cancel, waiting: !!pending || opening, error };
}
