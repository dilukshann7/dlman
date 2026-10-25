import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useWebMediaCapture } from './useWebMediaCapture';
import type { WebMediaExtensionContext } from '../lib/web-media-context';
import { awaitWebMediaInspection, isWebMediaUrl, sortWebMediaVariants, validateWebMediaSelection,
  webMediaSelection, webMediaSelectionForQuality, type WebMediaInspection, type WebMediaPreview, type WebMediaSelection } from '../../../../shared/web-media.mjs';

interface InspectionState {
  key: string;
  preview?: WebMediaPreview;
  inspectionId?: string;
  selection?: WebMediaSelection;
  file?: boolean;
  error?: string;
}

export function useWebMediaInspection(url: string, active: boolean, context?: WebMediaExtensionContext, initialSelection?: WebMediaSelection) {
  const enabled = active && isWebMediaUrl(url);
  const intakePageUrl = context?.expectVideo ? context.page_url || context.referrer || url : url;
  const capture = useWebMediaCapture(intakePageUrl, enabled);
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<InspectionState>({ key: '' });
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const sourceId = capture.sourceId;
  const captureId = sourceId ? capture.capture?.id : undefined;
  const transferUrl = intakePageUrl;
  const key = JSON.stringify([url, transferUrl, captureId, sourceId, active, attempt, context?.referrer, context?.cookies, context?.page_title, context?.expectVideo, context?.quality_hint, initialSelection]);
  const currentKey = useRef(key); currentKey.current = key;

  const inspect = useCallback(() => {
    const operation = generation.current;
    return awaitWebMediaInspection(
      () => invoke<WebMediaInspection>('inspect_web_media', {
        url: transferUrl,
        ...(captureId ? { capture_id: captureId, source_id: sourceId }
          : context ? { ...(context.expectVideo ? { master_url: context.master_url } : {}),
              referrer: context.referrer, cookies: context.cookies, page_title: context.page_title } : {}),
      }),
      (id) => invoke<WebMediaInspection>('get_web_media_inspection', { id }),
      { isCurrent: () => currentKey.current === key && operation === generation.current },
    );
  }, [transferUrl, captureId, sourceId, context, key]);

  useEffect(() => {
    const operation = ++generation.current;
    let cancelled = false;
    setState({ key });
    if (!enabled) { setLoading(false); return; }
    setLoading(true);
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const result = await inspect();
          if (cancelled || operation !== generation.current) return;
          if (result.status === 'file') {
            // A manifest passed by the extension cannot fall back to downloading its text.
            if (context?.expectVideo || captureId) throw new Error('No downloadable video found. Retry Detect in browser.');
            setState({ key, file: true, inspectionId: result.id });
          } else if (result.preview) {
            const preview = { ...result.preview, variants: sortWebMediaVariants(result.preview.variants) };
            const requested = !captureId && attempt === 0 ? initialSelection
              || (context?.quality_hint ? webMediaSelectionForQuality(preview, context.quality_hint) : undefined) : undefined;
            if (requested && !validateWebMediaSelection(preview, requested)) {
              throw new Error('Selected quality is no longer available. Retry the quality list.');
            }
            setState({ key, preview, inspectionId: result.id, selection: requested || webMediaSelection(preview, preview.variants[0]) });
          }
        } catch (failure) {
          if (!cancelled && operation === generation.current) setState({ key, error: failure instanceof Error ? failure.message : String(failure) });
        } finally {
          if (!cancelled && operation === generation.current) setLoading(false);
        }
      })();
    }, 500);
    return () => { cancelled = true; generation.current++; clearTimeout(timer); };
  }, [key, enabled, inspect, context, captureId, initialSelection]);

  const current = state.key === key ? state : { key };
  const choose = useCallback((id: string) => {
    setState((previous) => {
      const variant = previous.preview?.variants.find((v) => v.id === id);
      return previous.key === key && variant && previous.preview
        ? { ...previous, selection: webMediaSelection(previous.preview, variant) } : previous;
    });
  }, [key]);

  const refreshSelection = useCallback(async () => {
    if (!current.selection) throw new Error('Select a video format first.');
    const result = await inspect();
    if (!result.preview || !validateWebMediaSelection(result.preview, current.selection)) throw new Error('Selected quality is no longer available. Retry the quality list.');
    return result.id;
  }, [current.selection, inspect]);

  return { ...current, enabled, transferUrl, loading: enabled && (loading || state.key !== key), choose,
    refreshSelection, capture, retry: () => setAttempt((n) => n + 1) };
}
