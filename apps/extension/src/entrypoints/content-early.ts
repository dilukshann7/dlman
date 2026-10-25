import { defineContentScript } from 'wxt/sandbox';

/**
 * Early-injection content script (document_start).
 *
 * Injects the MAIN-world media hook script before any page JavaScript runs.
 * This ensures we intercept fetch/XHR/MSE calls from the very beginning.
 *
 * The actual hook logic lives in /media-hook.js (built from src/lib/media-hook.ts).
 * This script simply injects it into the page's MAIN world via a <script> tag.
 */
export default defineContentScript({
  matches: ['<all_urls>'],
  allFrames: true,
  matchAboutBlank: true,
  runAt: 'document_start',
  
  main() {
    try {
      const script = document.createElement('script');
      script.src = new URL('media-hook.js', browser.runtime.getURL('/')).href;
      script.onload = () => script.remove();
      const inject = () => (document.head || document.documentElement)?.appendChild(script);
      if (document.documentElement) inject();
      else {
        const observer = new MutationObserver(() => {
          if (document.documentElement) { observer.disconnect(); inject(); }
        });
        observer.observe(document, { childList: true });
      }
    } catch {
      // Silently fail — some pages may block script injection (CSP)
    }
  },
});
