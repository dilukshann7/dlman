import { canonicalYoutubeUrl, isYoutubeHost, awaitYoutubeInspection, type YoutubeSelection } from '../../../../shared/youtube.mjs';
import type { DetectedMedia, MediaDownloadRequest } from './media-types';
import { VideoOverlayManager } from './video-overlay';
import { disabledSitesStorage, settingsStorage } from './storage';

/** Each poll is a short request so Manifest V3 workers may safely suspend between polls. */
export async function resolveYoutubeMedia(media: DetectedMedia): Promise<DetectedMedia> {
  const canonical = canonicalYoutubeUrl(media.master_url);
  if (!canonical) throw new Error('Only individual public YouTube videos and Shorts are supported.');
  const inspection = await awaitYoutubeInspection(
    () => browser.runtime.sendMessage({ type: 'youtube-inspect', url: canonical }),
    (id) => browser.runtime.sendMessage({ type: 'youtube-inspection', inspection_id: id }),
    { isCurrent: () => canonicalYoutubeUrl(location.href) === canonical },
  );
  const preview = inspection.preview;
  if (preview.url !== canonical) throw new Error('Video changed.');
  return { ...media, inspection_id: inspection.id, page_title: preview.title,
    variants: preview.variants.map((variant) => {
      const selection: YoutubeSelection = { identity: preview.identity, format_id: variant.id,
        container: variant.container, quality: variant.quality, output_mode: variant.output_mode };
      return { url: canonical, label: variant.output_mode === 'mp3' ? variant.quality : `${variant.quality} · ${variant.container.toUpperCase()}`,
        audio_only: variant.output_mode === 'mp3', estimated_size: variant.size ?? undefined,
        youtube_selection: selection };
    }) };
}

/** Follow YouTube's client-side navigation and replace overlays when its player changes. */
export class YoutubeController {
  private timer: ReturnType<typeof setInterval>;
  private media?: DetectedMedia;
  private video?: HTMLVideoElement;
  private enabled = false;
  private unwatch: () => void;
  private destroyed = false;

  constructor(private overlay: VideoOverlayManager) {
    const refresh = async () => {
      const settings = await settingsStorage.get();
      this.enabled = settings.enabled && !(await disabledSitesStorage.isDisabled(location.hostname));
      if (!this.destroyed) this.tick();
    };
    this.unwatch = settingsStorage.watch(() => { void refresh(); });
    void refresh();
    this.timer = setInterval(() => this.tick(), 500);
  }

  private tick(): void {
    if (this.destroyed || !isYoutubeHost(location.href)) return;
    const canonical = this.enabled ? canonicalYoutubeUrl(location.href) : null;
    const video = document.querySelector<HTMLVideoElement>('ytd-reel-video-renderer[is-active] video')
      || document.querySelector<HTMLVideoElement>('#movie_player video');
    if (this.media && (this.media.master_url !== canonical || this.video !== video || !video?.isConnected)) {
      this.overlay.removeOverlay(this.media.id); this.media = undefined; this.video = undefined;
    }
    if (!canonical || !video || this.media) return;
    this.media = { id: `youtube:${canonical.split('v=')[1]}`, page_url: canonical,
      master_url: canonical, page_title: document.title, protocol: 'youtube', variants: [] };
    this.video = video;
    this.overlay.addOverlay(this.media, [], video);
  }

  async request(): Promise<MediaDownloadRequest | null> {
    this.tick();
    if (!this.media) return null;
    const media = await resolveYoutubeMedia(this.media);
    const first = media.variants.find((variant) => !variant.audio_only);
    if (!first) throw new Error('No video format is available. Use the player quality menu for MP3.');
    return { media, inspection_id: media.inspection_id, youtube_selection: first.youtube_selection };
  }

  destroy(): void {
    this.destroyed = true; clearInterval(this.timer); this.unwatch();
    if (this.media) this.overlay.removeOverlay(this.media.id);
  }
}
