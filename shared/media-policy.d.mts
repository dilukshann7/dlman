export function publicMediaUrl(value: unknown): URL | null;
export function isAdResource(value: string): boolean;
export function classifyMediaCandidate(candidate: { url: string; mime?: string; isAd?: boolean }): {
  url: string; protocol: 'direct' | 'hls' | 'dash';
} | null;
