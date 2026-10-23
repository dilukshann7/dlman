export interface YoutubeVariant {
  id: string;
  quality: string;
  container: string;
  audio: string;
  size: number | null;
  fps: number | null;
  output_mode: 'video' | 'mp3';
}
export interface YoutubeSelection {
  identity: string;
  format_id: string;
  container: string;
  quality: string;
  output_mode: 'video' | 'mp3';
}
export interface YoutubePreview {
  url: string;
  identity: string;
  title: string;
  variants: YoutubeVariant[];
}
export interface YoutubeInspection {
  id: string;
  status: 'pending' | 'ready' | 'failed';
  preview?: YoutubePreview | null;
  error?: string | null;
}
export function canonicalYoutubeUrl(value: string): string | null;
export function isYoutubeHost(value: string): boolean;
export function videoChoices<T extends { output_mode?: string }>(variants: T[]): T[];
export function awaitYoutubeInspection(
  start: () => Promise<YoutubeInspection>,
  poll: (id: string) => Promise<YoutubeInspection>,
  options?: { isCurrent?: () => boolean; delay?: () => Promise<void>; now?: () => number; timeout?: number }
): Promise<YoutubeInspection & { preview: YoutubePreview }>;
export function createYoutubeDialogQueue<T>(
  open: (payload: T) => void, isOpen: () => boolean, schedule?: (callback: () => void) => unknown
): { offer(payload: T): void; closed(): void; destroy(): void };
