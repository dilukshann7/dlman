export interface YoutubeSessionCookie {
  domain: string;
  path: string;
  name: string;
  value: string;
  secure: boolean;
  http_only: boolean;
  host_only: boolean;
  expires: number | null;
}
export interface YoutubeVerificationRequest { token: string; url: string; tabId: number; created: number }
export interface VerificationTab { url?: string; incognito?: boolean; cookieStoreId?: string }
export interface BrowserCookie {
  domain: string; path: string; name: string; value: string;
  secure?: boolean; httpOnly?: boolean; hostOnly?: boolean; expirationDate?: number;
}
export function parseYoutubeVerificationUrl(value: string): { token: string; url: string } | null;
export function youtubeSessionCookies(cookies: BrowserCookie[], now?: number): YoutubeSessionCookie[];
export function shareYoutubeSession<T>(request: YoutubeVerificationRequest, dependencies: {
  now?: () => number;
  getTab: (id: number) => Promise<VerificationTab>;
  getCookies: (details: { url: string; storeId?: string }) => Promise<BrowserCookie[]>;
  submit: (payload: { token: string; url: string; cookies: YoutubeSessionCookie[] }) => Promise<T>;
}): Promise<T>;
