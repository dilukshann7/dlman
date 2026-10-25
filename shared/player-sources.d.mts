import type { CaptureSource } from './web-media-capture.mjs';

export interface PlayerSource { src?: string; type?: string; label?: string; fps?: number; width?: number; height?: number }
export interface PlayerVideo extends PlayerSource {
  id?: string;
  configId?: string;
  setup?: string;
  currentSrc?: string;
  readyState?: number;
  sources?: PlayerSource[];
}
export interface PlayerSnapshot {
  baseUrl: string;
  title?: string;
  videos?: PlayerVideo[];
  scripts?: Array<string | { text: string; type?: string }>;
}
export type PlayerCandidate = CaptureSource & { label?: string; fps?: number; width?: number; height?: number };
export function collectPlayerSources(snapshot: PlayerSnapshot): PlayerCandidate[];
export function collectPlayerGroups(snapshot: PlayerSnapshot): Array<{ playerId: string; sources: PlayerCandidate[] }>;
