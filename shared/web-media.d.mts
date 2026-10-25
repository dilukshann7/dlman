export interface WebMediaVariant {
  id: string;
  quality: string;
  container: string;
  audio: string;
  size?: number | null;
  fps?: number | null;
  output_mode: 'video';
}
export interface WebMediaPreview {
  url: string;
  identity: string;
  title: string;
  variants: WebMediaVariant[];
}
export interface WebMediaSelection {
  identity: string;
  format_id: string;
  container: string;
  quality: string;
  output_mode: 'video';
}
export interface WebMediaQualityHint { label: string; height?: number; fps?: number }
export function webMediaSelectionForQuality(preview: WebMediaPreview, hint: WebMediaQualityHint): WebMediaSelection;
export interface WebMediaInspection {
  id: string;
  status: 'pending' | 'ready' | 'file' | 'failed';
  preview?: WebMediaPreview | null;
  error?: string | null;
}
export interface WebMediaSource {
  id: string;
  url: string;
  title?: string | null;
  protocol: 'direct' | 'hls' | 'dash';
}
export interface WebMediaCapture {
  id: string;
  url: string;
  status: 'waiting' | 'watching' | 'ready' | 'cancelled' | 'expired';
  sources: WebMediaSource[];
  error?: string | null;
  bridge_url?: string | null;
}
export function isWebMediaUrl(value: string): boolean;
export function sortWebMediaVariants<T extends WebMediaVariant>(variants: T[]): T[];
export function webMediaFilename(title: string, container: string, edited?: string): string;
export function webMediaSelection(preview: WebMediaPreview, variant: WebMediaVariant): WebMediaSelection;
export function validateWebMediaSelection(preview: WebMediaPreview, selection: WebMediaSelection): boolean;
export function awaitWebMediaInspection(start: () => Promise<WebMediaInspection>, poll: (id: string) => Promise<WebMediaInspection>, options?: {
  isCurrent?: () => boolean; delay?: () => Promise<void>; now?: () => number; timeout?: number;
}): Promise<WebMediaInspection>;
export function webMediaCaptureBridge(capture: Pick<WebMediaCapture, 'id' | 'bridge_url'>): string;
