import { useCallback, useEffect, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { canonicalYoutubeUrl, isYoutubeHost, awaitYoutubeInspection, type YoutubeInspection, type YoutubePreview, type YoutubeSelection } from '../../../../shared/youtube.mjs';

export function useYoutubeInspection(url: string, active: boolean, preferred?: YoutubeSelection) {
  const [preview, setPreview] = useState<YoutubePreview>();
  const [inspectionId, setInspectionId] = useState<string>();
  const [selection, setSelection] = useState<YoutubeSelection>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  const enabled = active && isYoutubeHost(url);
  const currentUrl = useRef(url); currentUrl.current = url;
  useEffect(() => {
    let cancelled = false;
    setPreview(undefined); setInspectionId(undefined); setSelection(undefined); setError(undefined);
    if (!enabled) { setLoading(false); return; }
    const canonical = canonicalYoutubeUrl(url);
    if (!canonical) { setLoading(false); setError('Only individual public YouTube videos and Shorts are supported.'); return; }
    setLoading(true);
    void (async () => {
      try {
        const result = await awaitYoutubeInspection(
          () => invoke<YoutubeInspection>('inspect_youtube', { url: canonical }),
          (id) => invoke<YoutubeInspection>('get_youtube_inspection', { id }),
          { isCurrent: () => !cancelled },
        );
        if (cancelled) return;
        const preview = result.preview;
        const variant = preferred && attempt === 0
          ? preview.variants.find((v) => preview.identity === preferred.identity && v.id === preferred.format_id && v.output_mode === preferred.output_mode && v.container === preferred.container)
          : preview.variants.find((v) => v.output_mode === 'video') || preview.variants[0];
        if (!variant) throw new Error('Selected format is unavailable. Refresh the quality list.');
        setPreview(preview); setInspectionId(result.id);
        setSelection({ identity: preview.identity, format_id: variant.id, container: variant.container, quality: variant.quality, output_mode: variant.output_mode });
      } catch (error) {
        if (!cancelled) setError(error instanceof Error ? error.message : String(error));
      } finally { if (!cancelled) setLoading(false); }
    })();
    return () => { cancelled = true; };
  }, [url, enabled, preferred, attempt]);

  const choose = useCallback((key: string) => {
    const variant = preview?.variants.find((v) => `${v.output_mode}:${v.id}` === key);
    if (variant && preview) setSelection({ identity: preview.identity, format_id: variant.id,
      container: variant.container, quality: variant.quality, output_mode: variant.output_mode });
  }, [preview]);
  const refreshSelection = useCallback(async () => {
    if (!selection) throw new Error('Select a video format first.');
    const canonical = canonicalYoutubeUrl(url);
    if (!canonical) throw new Error('Invalid YouTube video URL.');
    const result = await awaitYoutubeInspection(
      () => invoke<YoutubeInspection>('inspect_youtube', { url: canonical }),
      (id) => invoke<YoutubeInspection>('get_youtube_inspection', { id }),
      { isCurrent: () => currentUrl.current === url },
    );
    const matches = result.preview.identity === selection.identity && result.preview.variants.some((v) =>
      v.id === selection.format_id && v.container === selection.container && v.quality === selection.quality && v.output_mode === selection.output_mode);
    if (!matches) throw new Error('Selected quality is no longer available. Refresh the quality list.');
    setInspectionId(result.id);
    return result.id;
  }, [url, selection]);
  return { enabled, preview, inspectionId, selection, loading, error, choose, refreshSelection, retry: () => setAttempt((n) => n + 1) };
}
