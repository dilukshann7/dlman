import { awaitWebMediaInspection, sortWebMediaVariants, webMediaSelection, type WebMediaInspection } from '../../../../shared/web-media.mjs';
import type { DetectedMedia } from './media-types';

const documentId = crypto.randomUUID();
export function discoveryDocumentId(): string { return documentId; }

/** Automatic extraction shares no browser credentials and never opens a download dialog. */
export async function resolveWebpageMedia(media: DetectedMedia, retry = false, isCurrent = () => true): Promise<DetectedMedia> {
  if (document.visibilityState === 'hidden') throw Object.assign(new Error('Open this video tab to detect its sources.'), { deferred: true });
  const page = location.href;
  const request = async (again: boolean) => {
    const response = await browser.runtime.sendMessage({
    type: 'web-media-auto-inspect', document_id: documentId, player_id: media.id,
    url: media.extraction_url || media.page_url, retry: again,
    }) as WebMediaInspection & { deferred?: boolean };
    if (response?.deferred) throw Object.assign(new Error(response.error || 'Open this video tab.'), { deferred: true });
    return response;
  };
  const result = await awaitWebMediaInspection(() => request(retry), () => request(false),
    { isCurrent: () => location.href === page && !document.hidden && isCurrent() });
  if (!result?.preview || result.status !== 'ready') throw new Error(result?.error || 'No downloadable video found. Retry or start playback.');
  const preview = result.preview;
  return { ...media, protocol: 'webpage', discovery_status: 'ready', discovery_error: undefined,
    inspection_id: result.id, page_title: preview.title,
    variants: sortWebMediaVariants(preview.variants).map((variant) => ({
      url: preview.url, label: variant.quality + ' · ' + variant.container.toUpperCase(),
      fps: variant.fps ?? undefined, estimated_size: variant.size ?? undefined,
      web_media_selection: webMediaSelection(preview, variant),
    })),
  };
}
