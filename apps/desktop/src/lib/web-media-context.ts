import type { WebMediaQualityHint } from '../../../../shared/web-media.mjs';

export interface WebMediaExtensionContext {
  quality_hint?: WebMediaQualityHint;
  master_url: string;
  referrer?: string;
  cookies?: string;
  page_title?: string;
  /** Original webpage for a stream source; transient and never persisted. */
  page_url?: string;
  /** Frontend classification policy; never forwarded to the native command. */
  expectVideo: boolean;
}

interface BrowserIntake {
  cookies?: string;
  sourcePageUrl?: string;
  media?: { protocol: string; master_url: string; referrer?: string; page_title?: string; media_quality?: WebMediaQualityHint };
}

/** Browser credentials belong only to the URL originally delivered by its intake. */
export function webMediaExtensionContext(url: string, intakeUrl: string | undefined, intake: BrowserIntake): WebMediaExtensionContext | undefined {
  if (url !== intakeUrl) return undefined;
  const stream = intake.media && ['hls', 'dash'].includes(intake.media.protocol) ? intake.media : undefined;
  if (!stream && !intake.cookies && !intake.sourcePageUrl) return undefined;
  return {
    master_url: stream?.master_url || url,
    referrer: stream?.referrer || intake.sourcePageUrl,
    cookies: intake.cookies,
    page_title: stream?.page_title,
    ...(stream ? { page_url: intake.sourcePageUrl || stream.referrer } : {}),
    expectVideo: !!stream,
    ...(stream?.media_quality ? { quality_hint: stream.media_quality } : {}),
  };
}
