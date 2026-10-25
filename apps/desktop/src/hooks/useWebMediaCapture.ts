import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-shell';
import { webMediaCaptureBridge, type WebMediaCapture } from '../../../../shared/web-media.mjs';

const cancelTicket = (id: string) => { void invoke('cancel_web_media_capture', { id }).catch(() => {}); };

export function useWebMediaCapture(url: string, active: boolean) {
  const [capture, setCapture] = useState<WebMediaCapture>();
  const [sourceId, setSourceId] = useState<string>();
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string>();
  const [extensionMissing, setExtensionMissing] = useState(false);
  const generation = useRef(0);
  const explicitSource = useRef<string>();
  const ticket = useRef<string>();
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const current = useRef({ url, active });
  current.current = { url, active };

  const stop = useCallback(() => {
    generation.current++;
    clearTimeout(timer.current);
    timer.current = undefined;
    if (ticket.current) cancelTicket(ticket.current);
    ticket.current = undefined;
  }, []);

  useEffect(() => {
    setCapture(undefined); setSourceId(undefined); setError(undefined);
    setOpening(false); setExtensionMissing(false);
    return stop;
  }, [url, active, stop]);

  const begin = useCallback(async () => {
    if (!active) return;
    stop();
    const attempt = generation.current;
    const isCurrent = () => attempt === generation.current && current.current.active && current.current.url === url;
    const started = Date.now();
    explicitSource.current = undefined;
    setCapture(undefined); setSourceId(undefined); setError(undefined);
    setExtensionMissing(false); setOpening(true);
    let request: WebMediaCapture | undefined;
    try {
      request = await invoke<WebMediaCapture>('begin_web_media_capture', { url });
      if (!isCurrent()) { cancelTicket(request.id); return; }
      ticket.current = request.id;
      await open(webMediaCaptureBridge(request));
      if (!isCurrent()) return;
      setCapture(request);
      const id = request.id;
      const poll = async () => {
        try {
          const result = await invoke<WebMediaCapture>('get_web_media_capture', { id });
          if (!isCurrent()) return;
          setCapture((previous) => JSON.stringify(previous) === JSON.stringify(result) ? previous : result);
          setExtensionMissing(result.status === 'waiting' && Date.now() - started >= 15_000);
          if (result.status === 'ready') {
            if (explicitSource.current && !result.sources.some((source) => source.id === explicitSource.current)) explicitSource.current = undefined;
            if (!explicitSource.current) setSourceId(result.sources.length === 1 ? result.sources[0].id : undefined);
            if (!result.sources.length) setError('No video sources detected. Play the video, then retry Detect in browser.');
          }
          if (result.status === 'cancelled' || result.status === 'expired') {
            setSourceId(undefined);
            setError(result.error || 'Browser detection ended. Retry Detect in browser.');
            return;
          }
          if (Date.now() - started >= 600_000) {
            cancelTicket(id);
            setCapture({ ...result, status: 'expired' });
            setSourceId(undefined);
            setError('Browser detection expired after 10 minutes. Retry Detect in browser.');
            return;
          }
          timer.current = setTimeout(() => { void poll(); }, 1000);
        } catch (failure) {
          if (isCurrent()) {
            cancelTicket(id);
            setCapture(undefined);
            setError(String(failure));
          }
        }
      };
      void poll();
    } catch (failure) {
      if (request) cancelTicket(request.id);
      if (isCurrent()) { ticket.current = undefined; setError(String(failure)); }
    } finally {
      if (isCurrent()) setOpening(false);
    }
  }, [url, active, stop]);

  const cancel = useCallback(() => {
    stop(); setCapture(undefined); setSourceId(undefined); setOpening(false);
    setError(undefined); setExtensionMissing(false);
  }, [stop]);

  const chooseSource = useCallback((id: string) => {
    if (capture?.status === 'ready' && capture.sources.some((source) => source.id === id)) {
      explicitSource.current = id;
      setSourceId(id);
    }
  }, [capture]);
  const waiting = opening || capture?.status === 'waiting' || capture?.status === 'watching';
  return { capture, sourceId, begin, cancel, chooseSource, waiting, error, extensionMissing };
}
