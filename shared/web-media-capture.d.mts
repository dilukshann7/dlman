export const CAPTURE_TTL: number;
export interface CaptureMetadata { id: string; client_id: string; tab_id: number; url: string; port: number; created: number }
export interface CaptureCandidate { url: string; title?: string; mime?: string; protocol?: 'direct' | 'hls' | 'dash'; referrer?: string; user_agent?: string }
export interface CaptureSource extends CaptureCandidate { protocol: 'direct' | 'hls' | 'dash' }
export interface CaptureTab { id?: number; url?: string; incognito?: boolean; cookieStoreId?: string }
export interface BrowserCookie { domain: string; path: string; name: string; value: string; secure?: boolean; httpOnly?: boolean; hostOnly?: boolean; expirationDate?: number }
export interface ScopedCookie { domain: string; path: string; name: string; value: string; secure: boolean; http_only: boolean; host_only: boolean; expires: number | null }
export interface CaptureSubmission { client_id: string; tab_id: number; page_url: string; sources: Array<CaptureSource & { referrer: string; user_agent?: string; cookies: ScopedCookie[] }> }
export function parseCaptureBridge(value: string, expectedPort: number): { id: string; port: number } | null;
export function claimBrowserCapture(sender: { id?: string; url?: string; frameId?: number; tab?: { id?: number } }, dependencies: {
  extensionId: string; port: number; now?: () => number; newClientId(): string;
  getTab(id: number): Promise<CaptureTab>; isAllowed(url: string): Promise<boolean>;
  claim(bridge: { id: string; port: number }, claim: { client_id: string; tab_id: number }): Promise<{ id: string; url: string; status: string }>;
  cancel(bridge: { id: string; port: number }, claim: { client_id: string; tab_id: number }): Promise<unknown>;
}): Promise<CaptureMetadata>;
export function captureTabMatches(request: CaptureMetadata, tab: CaptureTab, now?: number): boolean;
export function captureSource(candidate: CaptureCandidate): CaptureSource | null;
export function mergeCaptureSources(existing: CaptureCandidate[], candidates: CaptureCandidate[]): CaptureSource[];
export function scopedMediaCookies(cookies: BrowserCookie[], url: string, now?: number): ScopedCookie[];
export function submitCapturedSources(request: CaptureMetadata, candidates: CaptureCandidate[], dependencies: {
  now?: () => number;
  getTab(id: number): Promise<CaptureTab>;
  isAllowed(url: string): Promise<boolean>;
  getCookies(details: { url: string; storeId?: string }): Promise<BrowserCookie[]>;
  submit(payload: CaptureSubmission): Promise<unknown>;
  userAgent?: string;
}): Promise<unknown>;
