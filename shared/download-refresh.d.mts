export function canRefreshDownload(download: { url: string; status: string; youtube?: unknown; web_media?: unknown } | null | undefined): boolean;
export function refreshSessionExpired(session: { expires_at: number } | null | undefined, now?: number): boolean;
export function displayRefreshUrl(value: string): string;
