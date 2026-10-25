import { classifyMediaCandidate } from '../../../../shared/media-policy.mjs';
import { handleAutomaticMedia, forgetAutomaticMediaTab, resetAutomaticMedia } from '@/lib/automatic-media';
import { canonicalYoutubeUrl, isYoutubeHost } from '../../../../shared/youtube.mjs';
import { handleYoutubeVerificationMessage } from '@/lib/youtube-verification';
import { handleWebMediaCaptureMessage, setupWebMediaCapture } from '@/lib/web-media-capture';
import { defineBackground } from 'wxt/sandbox';
import { getDlmanClient, resetDlmanClient } from '@/lib/api-client';
import type { WsEvent } from '@/lib/api-client';
import { settingsStorage, disabledSitesStorage } from '@/lib/storage';
import { isDownloadableUrl, extractFilename } from '@/lib/utils';
import type { ExtensionSettings } from '@/types';
import type { MediaDownloadRequest, DetectedMedia } from '@/lib/media-types';

export default defineBackground(() => {
  console.log('[DLMan] Background service worker started');
  setupWebMediaCapture();

  // Compat shim: browser.action (MV3) vs browser.browserAction (Firefox MV2)
  const actionApi = browser.action ?? (browser as any).browserAction;

  let currentSettings: ExtensionSettings | null = null;
  let connectionStatus: 'connected' | 'disconnected' | 'connecting' = 'disconnected';

  // Name for the periodic reconnection alarm
  const RECONNECT_ALARM = 'dlman-reconnect';

  // ============================================================================
  // Initialization
  // ============================================================================

  async function init() {
    // Load settings
    currentSettings = await settingsStorage.get();
    console.log('[DLMan] Settings loaded:', currentSettings);

    // Set up context menus
    await setupContextMenus();

    // Watch for setting changes
    settingsStorage.watch((settings) => {
      if (settings) {
        const portChanged = currentSettings?.port !== settings.port;
        currentSettings = settings;
        resetAutomaticMedia();

        // Reconnect if port changed
        if (portChanged) {
          resetDlmanClient();
          connectToDlman();
        }

        updateBadge();
      }
    });

    // Set up periodic reconnection via alarms API
    setupReconnectAlarm();

    // Try to connect to DLMan
    await connectToDlman();

    // Set up download interception
    setupDownloadInterception();

    // Set up webRequest-based stream detection
    setupStreamDetection();

    // Update badge
    updateBadge();
  }

  // ============================================================================
  // Periodic reconnection via browser.alarms
  // ============================================================================

  function setupReconnectAlarm() {
    // Create alarm that fires every 30 seconds
    browser.alarms.create(RECONNECT_ALARM, { periodInMinutes: 0.5 });

    browser.alarms.onAlarm.addListener(async (alarm) => {
      if (alarm.name !== RECONNECT_ALARM) return;

      // Only reconnect if enabled and currently disconnected
      if (!currentSettings?.enabled || connectionStatus === 'connected') return;

      console.log('[DLMan] Alarm: attempting reconnection...');
      await connectToDlman();
    });
  }

  // ============================================================================
  // DLMan Connection
  // ============================================================================

  let automaticConnected = false;
  function resumeAutomaticMedia() {
    if (automaticConnected) return;
    automaticConnected = true;
    resetAutomaticMedia();
    void browser.tabs.query({}).then((tabs) => {
      for (const tab of tabs) if (tab.id != null) {
        browser.tabs.sendMessage(tab.id, { type: 'media-discovery-retry' }).catch(() => {});
      }
    });
  }

  async function connectToDlman() {
    if (!currentSettings?.enabled) {
      connectionStatus = 'disconnected';
      updateBadge();
      return;
    }

    connectionStatus = 'connecting';
    updateBadge();

    const client = getDlmanClient({
      port: currentSettings.port,
      onConnect: () => {
        resumeAutomaticMedia();
        connectionStatus = 'connected';
        updateBadge();
        console.log('[DLMan] Connected to desktop app');
      },
      onDisconnect: () => {
        automaticConnected = false;
        connectionStatus = 'disconnected';
        updateBadge();
        console.log('[DLMan] Disconnected from desktop app');
      },
      onEvent: (event: WsEvent) => {
        // Broadcast real-time events to popup if open
        if (event.type === 'progress' && event.id) {
          browser.runtime.sendMessage({
            type: 'download_progress',
            payload: {
              id: event.id,
              downloaded: event.downloaded ?? 0,
              total: event.total ?? null,
              speed: event.speed ?? 0,
              eta: event.eta ?? null,
            },
          }).catch(() => {
            // Popup not open, ignore
          });
        } else if (event.type === 'status_changed' || event.type === 'download_added') {
          // Tell popup to refresh its data
          browser.runtime.sendMessage({
            type: 'data_changed',
          }).catch(() => {});
        }
      },
      onError: (error: string) => {
        console.error('[DLMan] Server error:', error);
      },
    });

    // First check if the app is even running
    const isRunning = await client.ping();
    if (!isRunning) {
      automaticConnected = false;
      connectionStatus = 'disconnected';
      updateBadge();
      console.log('[DLMan] Desktop app not running');
      return;
    }

    // App is running — try to open WebSocket for real-time events
    const wsConnected = await client.connect();
    if (wsConnected) {
      connectionStatus = 'connected';
    } else {
      // WS failed but HTTP ping works — mark as connected (HTTP-only mode)
      connectionStatus = 'connected';
      console.log('[DLMan] Connected via HTTP only (WebSocket unavailable)');
    }
    updateBadge();
  }

  // ============================================================================
  // Context Menu
  // ============================================================================

  async function setupContextMenus() {
    await browser.contextMenus.removeAll();

    browser.contextMenus.create({
      id: 'download-with-dlman',
      title: 'Download with DLMan',
      contexts: ['link', 'video', 'audio', 'image'],
    });

    // "Download Video" — appears when right-clicking near a video
    browser.contextMenus.create({
      id: 'download-video-dlman',
      title: 'Download Video with DLMan',
      contexts: ['video', 'page'],
    });

    browser.contextMenus.create({
      id: 'download-selected-links',
      title: 'Download selected links with DLMan',
      contexts: ['selection'],
    });

    browser.contextMenus.create({
      id: 'download-all-links',
      title: 'Download all links with DLMan',
      contexts: ['page'],
    });

    browser.contextMenus.create({
      id: 'separator-1',
      type: 'separator',
      contexts: ['page', 'link', 'selection'],
    });

    browser.contextMenus.create({
      id: 'toggle-site',
      title: 'Disable DLMan on this site',
      contexts: ['page', 'link'],
    });
  }

  browser.contextMenus.onClicked.addListener(async (info, tab) => {
    switch (info.menuItemId) {
      case 'download-with-dlman':
        if (info.linkUrl) {
          await handleDownload(info.linkUrl, tab?.url);
          if (tab?.id) {
            browser.tabs.sendMessage(tab.id, { type: 'show-toast', count: 1 }).catch(() => {});
          }
        } else if (info.srcUrl) {
          await handleDownload(info.srcUrl, tab?.url);
          if (tab?.id) {
            browser.tabs.sendMessage(tab.id, { type: 'show-toast', count: 1 }).catch(() => {});
          }
        }
        break;

      case 'download-video-dlman':
        if (tab?.url && isYoutubeHost(tab.url)) {
          const canonical = canonicalYoutubeUrl(tab.url);
          if (canonical) await handleDownload(canonical);
          return;
        }
        // Ask content script for the detected video near the click point
        if (tab?.id) {
          try {
            // Read stored right-click coordinates from content script
            const stored = await browser.storage.local.get('lastRightClick') as { lastRightClick?: { x: number; y: number } };
            const x = stored?.lastRightClick?.x ?? 0;
            const y = stored?.lastRightClick?.y ?? 0;

            const resp = await browser.tabs.sendMessage(tab.id, {
              type: 'get-video-at-point',
              x,
              y,
            }, { frameId: info.frameId ?? 0 }) as { request?: MediaDownloadRequest };
            if (resp?.request) {
              const client = getDlmanClient();
              const isAvailable = await client.ping();
              if (isAvailable) {
                const request = resp.request;
                if (request.media.protocol === 'webpage' && request.inspection_id && request.web_media_selection) {
                  await client.downloadWebMedia({ url: request.media.extraction_url || request.media.page_url,
                    inspection_id: request.inspection_id, selection: request.web_media_selection });
                } else if (request.media.protocol !== 'webpage') await client.downloadMedia(request);
              } else if (resp.request.media.protocol === 'direct') {
                await handleDownload(
                  resp.request.media.master_url,
                  resp.request.media.referrer,
                  resp.request.media.filename || undefined,
                );
              }
              browser.tabs.sendMessage(tab.id, { type: 'show-toast', count: 1 }).catch(() => {});
            } else {
              // No detected media — try srcUrl if it's a real media URL
              // (skip blob:, data:, and page URLs which are useless)
              const src = info.srcUrl;
              if (src && !src.startsWith('blob:') && !src.startsWith('data:') && !src.startsWith('http://www.') && !src.startsWith('https://www.') && /\.(mp4|webm|m3u8|mpd|mkv|mov|m4v|flv)/i.test(src)) {
                await handleDownload(src, tab.url);
                browser.tabs.sendMessage(tab.id, { type: 'show-toast', count: 1 }).catch(() => {});
              }
            }
          } catch {
            // Content script not ready — fall back to srcUrl only if it's a media URL
            const src = info.srcUrl;
            if (src && !src.startsWith('blob:') && !src.startsWith('data:') && /\.(mp4|webm|m3u8|mpd|mkv|mov|m4v|flv)/i.test(src)) {
              await handleDownload(src, tab?.url);
            }
          }
        }
        break;

      case 'download-selected-links':
        if (tab?.id) {
          try {
            const response = await browser.tabs.sendMessage(tab.id, { type: 'get-selected-links' }) as { links?: string[] };
            const links = response?.links || [];
            if (links.length === 0) {
              browser.notifications.create({
                type: 'basic',
                iconUrl: 'icon/128.png',
                title: 'DLMan',
                message: 'No links found in selection',
              });
            } else if (links.length === 1) {
              await handleDownload(links[0], tab.url);
              browser.tabs.sendMessage(tab.id, { type: 'show-toast', count: 1 }).catch(() => {});
            } else {
              await handleBatchDownload(links, tab.url);
              browser.tabs.sendMessage(tab.id, { type: 'show-toast', count: links.length }).catch(() => {});
            }
          } catch (error) {
            console.error('[DLMan] Failed to get selected links:', error);
          }
        }
        break;

      case 'download-all-links':
        if (tab?.id) {
          browser.tabs.sendMessage(tab.id, { type: 'get-all-links' });
        }
        break;

      case 'toggle-site':
        if (tab?.url) {
          try {
            const hostname = new URL(tab.url).hostname;
            const isNowDisabled = await disabledSitesStorage.toggle(hostname);

            if (currentSettings?.showNotifications) {
              browser.notifications.create({
                type: 'basic',
                iconUrl: 'icon/128.png',
                title: 'DLMan',
                message: isNowDisabled
                  ? `Disabled on ${hostname}`
                  : `Enabled on ${hostname}`,
              });
            }
          } catch (error) {
            console.error('[DLMan] Failed to toggle site:', error);
          }
        }
        break;
    }
  });

  // ============================================================================
  // Download Interception
  // ============================================================================

  const DOWNLOADABLE_MIME_TYPES = [
    'application/zip',
    'application/x-rar-compressed',
    'application/x-7z-compressed',
    'application/x-tar',
    'application/gzip',
    'application/x-bzip2',
    'application/octet-stream',
    'application/x-msdownload',
    'application/x-msi',
    'application/x-apple-diskimage',
    'application/x-iso9660-image',
    'video/mp4',
    'video/x-matroska',
    'video/x-msvideo',
    'video/quicktime',
    'video/webm',
    'audio/mpeg',
    'audio/flac',
    'audio/wav',
    'audio/mp4',
    'audio/aac',
    'audio/ogg',
    'application/pdf',
    'application/vnd.ms-excel',
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    'application/msword',
    'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    'application/vnd.ms-powerpoint',
    'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  ];

  function isDownloadableMimeType(mimeType: string | undefined): boolean {
    if (!mimeType) return false;
    const lowerMime = mimeType.toLowerCase().split(';')[0].trim();
    return DOWNLOADABLE_MIME_TYPES.some(
      (mime) => lowerMime === mime || lowerMime.startsWith(mime + ';'),
    );
  }

  function setupDownloadInterception() {
    browser.downloads.onCreated.addListener(async (downloadItem) => {
      if (!currentSettings?.enabled || !currentSettings?.autoIntercept) {
        return;
      }

      const url = downloadItem.url;
      const matchesPattern = url && isDownloadableUrl(url, currentSettings.interceptPatterns);
      const matchesMimeType = isDownloadableMimeType(downloadItem.mime);

      if (!url || (!matchesPattern && !matchesMimeType)) {
        return;
      }

      // Check if site is disabled
      try {
        const hostname = new URL(downloadItem.referrer || url).hostname;
        const isDisabled = await disabledSitesStorage.isDisabled(hostname);
        if (isDisabled) return;
      } catch {
        // Invalid URL, skip
      }

      // Check if DLMan is available
      const client = getDlmanClient();
      const isAvailable = await client.ping();

      if (!isAvailable) {
        if (currentSettings.fallbackToBrowser) {
          console.log('[DLMan] App not running, using browser download');
          return;
        }
        browser.notifications.create({
          type: 'basic',
          iconUrl: 'icon/128.png',
          title: 'DLMan Not Running',
          message: 'Start DLMan to intercept downloads',
        });
        return;
      }

      // Cancel browser download and hand off to DLMan
      try {
        await browser.downloads.cancel(downloadItem.id);
        await browser.downloads.erase({ id: downloadItem.id });
      } catch (error) {
        console.error('[DLMan] Failed to cancel browser download:', error);
      }

      await handleDownload(url, downloadItem.referrer, downloadItem.filename);

      // Show in-page toast so the user knows the download was redirected
      showToastInTab(1);
    });
  }

  // ============================================================================
  // Stream Detection via webRequest — catches HLS / DASH / direct video URLs
  // that are loaded by the page's JS (invisible to content script).
  //
  // Confirm successful responses before reporting candidates. URL extensions
  // and response MIME types share the same ad/transport-resource classifier.
  // ============================================================================

  // --- MIME-based detection ---
  interface StreamHint { url: string; protocol: string; documentId?: string }
  const sentUrls = new Map<string, Set<string>>();
  const pendingStreamUrls = new Map<string, StreamHint[]>();
  const frameDocuments = new Map<string, string>();
  const frameKey = (tabId: number, frameId: number) => tabId + ':' + frameId;

  function notifyContentScript(tabId: number, frameId: number, item: StreamHint): void {
    const key = frameKey(tabId, frameId);
    const identity = (item.documentId || '') + ':' + item.url;
    let seen = sentUrls.get(key);
    if (!seen) { seen = new Set(); sentUrls.set(key, seen); }
    if (seen.has(identity)) return;
    seen.add(identity);
    if (seen.size > 128) seen.delete(seen.values().next().value!);
    browser.tabs.sendMessage(tabId, { type: 'stream-detected', ...item }, { frameId }).catch(() => {
      seen!.delete(identity);
      const pending = pendingStreamUrls.get(key) || [];
      if (!pending.some((entry) => entry.url === item.url && entry.documentId === item.documentId)) pending.push(item);
      pendingStreamUrls.set(key, pending.slice(-64));
    });
  }

  function setupStreamDetection(): void {
    const clearTab = (tabId: number) => {
      for (const map of [sentUrls, pendingStreamUrls, frameDocuments]) {
        for (const key of map.keys()) if (key.startsWith(tabId + ':')) map.delete(key);
      }
      forgetAutomaticMediaTab(tabId);
    };
    browser.tabs.onRemoved.addListener(clearTab);
    browser.tabs.onUpdated.addListener((tabId, info) => { if (info.status === 'loading') clearTab(tabId); });
    browser.webRequest.onHeadersReceived.addListener((details) => {
      if (details.tabId < 0 || details.frameId < 0 || (details.statusCode !== 304 && (details.statusCode < 200 || details.statusCode >= 300))) return;
      if (!currentSettings?.enabled) return;
      const mime = details.responseHeaders?.find((header) => header.name.toLowerCase() === 'content-type')?.value;
      const candidate = classifyMediaCandidate({ url: details.url, mime });
      if (!candidate) return;
      const documentId = (details as typeof details & { documentId?: string }).documentId;
      notifyContentScript(details.tabId, details.frameId, { ...candidate, documentId });
    }, { urls: ['<all_urls>'] }, ['responseHeaders']);
  }

  // ============================================================================
  // Download Handler — opens dialog in desktop app, NEVER auto-starts
  // ============================================================================

  /**
   * Send a toast to the active tab's content script.
   */
  function showToastInTab(count: number) {
    // Get the active tab and send a toast message
    browser.tabs.query({ active: true, currentWindow: true }).then((tabs) => {
      const tabId = tabs[0]?.id;
      if (tabId) {
        browser.tabs.sendMessage(tabId, { type: 'show-toast', count }).catch(() => {});
      }
    }).catch(() => {});
  }

  /**
   * Read browser cookies for a URL's domain and format as HTTP Cookie header.
   * Returns `name1=value1; name2=value2` string, or undefined if no cookies.
   */
  async function getCookiesForUrl(url: string): Promise<string | undefined> {
    try {
      const cookies = await browser.cookies.getAll({ url });
      if (!cookies || cookies.length === 0) return undefined;

      // Format as standard HTTP Cookie header
      const cookieStr = cookies
        .map((c) => `${c.name}=${c.value}`)
        .join('; ');

      return cookieStr || undefined;
    } catch {
      console.error('[DLMan] Failed to read browser cookies.');
      return undefined;
    }
  }

  async function handleDownload(url: string, referrer?: string, suggestedFilename?: string) {
    const client = getDlmanClient();

    // Check if DLMan is available
    const isAvailable = await client.ping();
    if (!isAvailable) {
      if (currentSettings?.fallbackToBrowser) {
        browser.downloads.download({ url });
        return;
      }
      browser.notifications.create({
        type: 'basic',
        iconUrl: 'icon/128.png',
        title: 'DLMan Not Running',
        message: 'Please start DLMan to download files',
      });
      return;
    }

    // Read browser cookies for this domain — enables session-authenticated downloads
    const cookies = isYoutubeHost(url) ? undefined : await getCookiesForUrl(url);

    // Auto-detect HLS/DASH streaming URLs and route through the media pipeline
    // so the desktop app receives full media context (protocol, page title, etc.)
    const urlPath = url.split('?')[0].toLowerCase();
    const isHls = urlPath.endsWith('.m3u8') || urlPath.includes('.m3u8/');
    const isDash = urlPath.endsWith('.mpd') || urlPath.includes('.mpd/');

    if (isHls || isDash) {
      // Get page title from active tab for better filename
      let pageTitle: string | undefined;
      try {
        const tabs = await browser.tabs.query({ active: true, currentWindow: true });
        if (tabs[0]?.title) pageTitle = tabs[0].title;
      } catch { /* ignore */ }

      const result = await client.downloadMedia({
        media: {
          id: `manual-${Date.now()}`,
          page_url: referrer || '',
          page_title: pageTitle,
          master_url: url,
          protocol: isHls ? 'hls' : 'dash',
          variants: [],
          filename: suggestedFilename || undefined,
          cookies: cookies || undefined,
          referrer: referrer || undefined,
        },
        variant_index: undefined,
      });

      if (!result.success) {
        browser.notifications.create({
          type: 'basic',
          iconUrl: 'icon/128.png',
          title: 'DLMan Error',
          message: result.error || 'Failed to start media download',
        });
      }
      return;
    }

    // Regular file — open the download dialog in the desktop app
    const result = await client.showDialog({
      url,
      filename: suggestedFilename || extractFilename(url),
      referrer,
      cookies,
    });

    if (!result.success) {
      browser.notifications.create({
        type: 'basic',
        iconUrl: 'icon/128.png',
        title: 'DLMan Error',
        message: result.error || 'Failed to open download dialog',
      });
    }
  }

  /**
   * Handle bulk download — sends all URLs to the desktop app's batch dialog.
   */
  async function handleBatchDownload(urls: string[], referrer?: string) {
    const client = getDlmanClient();

    const isAvailable = await client.ping();
    if (!isAvailable) {
      if (currentSettings?.fallbackToBrowser) {
        // Fallback: download each one via browser
        for (const url of urls) {
          browser.downloads.download({ url });
        }
        return;
      }
      browser.notifications.create({
        type: 'basic',
        iconUrl: 'icon/128.png',
        title: 'DLMan Not Running',
        message: 'Please start DLMan to download files',
      });
      return;
    }

    const result = await client.showBatchDialog({ urls, referrer });

    if (!result.success) {
      browser.notifications.create({
        type: 'basic',
        iconUrl: 'icon/128.png',
        title: 'DLMan Error',
        message: result.error || 'Failed to open batch download dialog',
      });
    }
  }

  // ============================================================================
  // Badge Management
  // ============================================================================

  function updateBadge() {
    if (!actionApi) return; // Safety: no badge API available

    if (!currentSettings?.enabled) {
      actionApi.setBadgeText({ text: 'OFF' });
      actionApi.setBadgeBackgroundColor({ color: '#6b7280' });
      return;
    }

    switch (connectionStatus) {
      case 'connected':
        actionApi.setBadgeText({ text: '' });
        break;
      case 'connecting':
        actionApi.setBadgeText({ text: '...' });
        actionApi.setBadgeBackgroundColor({ color: '#f59e0b' });
        break;
      case 'disconnected':
        actionApi.setBadgeText({ text: '!' });
        actionApi.setBadgeBackgroundColor({ color: '#ef4444' });
        break;
    }
  }

  // ============================================================================
  // Message Handling (from popup, content scripts)
  // ============================================================================

  interface Message {
    type: string;
    url?: string;
    referrer?: string;
    links?: string[];
    media?: DetectedMedia;
    inspection_id?: string;
    request?: MediaDownloadRequest;
    document_id?: string;
    player_id?: string;
    retry?: boolean;
    eligible?: boolean;
  }

  browser.runtime.onMessage.addListener((message: unknown, sender, sendResponse) => {
    const msg = message as Message;

    switch (msg.type) {
      case 'web-media-capture-claim':
      case 'web-media-capture-active':
      case 'web-media-capture-sources':
        void handleWebMediaCaptureMessage(msg, sender).then(sendResponse).catch(() => {
          sendResponse({ active: false, success: false, error: 'Browser capture is unavailable or expired. Start again in DLMan.' });
        });
        return true;
      case 'get-status':
        (async () => {
          const client = getDlmanClient({
            port: currentSettings?.port || 7899,
          });
          const isAvailable = await client.ping();

          // Sync connection status based on actual reachability
          if (isAvailable && connectionStatus !== 'connected') {
            connectionStatus = 'connected';
            updateBadge();
          } else if (!isAvailable && connectionStatus === 'connected') {
            connectionStatus = 'disconnected';
            updateBadge();
          }

          sendResponse({
            enabled: currentSettings?.enabled,
            connected: isAvailable,
            connectionStatus: isAvailable ? 'connected' : connectionStatus,
          });
        })();
        return true;

      case 'add-download':
        (async () => {
          try {
            const client = getDlmanClient();
            const isAvailable = await client.ping();

            if (!isAvailable) {
              sendResponse({ success: false, error: 'DLMan is not running' });
              return;
            }

            // Open dialog in the desktop app (never auto-start)
            const result = await client.showDialog({
              url: msg.url || '',
              referrer: msg.referrer,
            });

            sendResponse({
              success: result.success,
              error: result.error,
            });
          } catch (error) {
            sendResponse({ success: false, error: (error as Error).message });
          }
        })();
        return true;

      case 'connect':
        (async () => {
          await connectToDlman();
          sendResponse({ connected: connectionStatus === 'connected' });
        })();
        return true;

      case 'all-links':
        // Handle links from content script — open batch dialog
        if (msg.links && Array.isArray(msg.links) && msg.links.length > 0) {
          handleBatchDownload(msg.links, sender.tab?.url).then(() => {
            if (sender.tab?.id) {
              browser.tabs.sendMessage(sender.tab.id, { type: 'show-toast', count: msg.links!.length }).catch(() => {});
            }
          });
        }
        return true;

      // ==================================================================
      // Media detection & download (from content script video detector)
      // ==================================================================

      case 'content-script-ready': {
        if (sender.tab?.id != null && sender.frameId != null) {
          const key = frameKey(sender.tab.id, sender.frameId);
          const documentId = (sender as typeof sender & { documentId?: string }).documentId || '';
          const previous = frameDocuments.get(key);
          if (previous && previous !== documentId) sentUrls.delete(key);
          frameDocuments.set(key, documentId);
          const pending = pendingStreamUrls.get(key) || [];
          pendingStreamUrls.delete(key);
          for (const item of pending) {
            if (!item.documentId || !documentId || item.documentId === documentId) notifyContentScript(sender.tab.id, sender.frameId, item);
          }
          sendResponse({ ok: true, documentId });
        } else sendResponse({ ok: true });
        return true;
      }

      case 'media-discovery-ready':
      case 'media-discovery-stop':
      case 'media-discovery-player':
      case 'web-media-auto-inspect':
        void handleAutomaticMedia(msg, sender).then(sendResponse).catch((error) => {
          sendResponse({ status: 'failed', deferred: !!error?.deferred, error: error instanceof Error ? error.message : 'Video inspection is unavailable.' });
        });
        return true;

      case 'youtube-inspect':
      case 'youtube-inspection':
        (async () => {
          try {
            const client = getDlmanClient();
            const result = msg.type === 'youtube-inspect'
              ? await client.inspectYoutube(msg.url || '')
              : await client.youtubeInspection(msg.inspection_id || '');
            sendResponse(result);
          } catch (error) {
            sendResponse({ status: 'failed', error: error instanceof Error ? error.message : 'DLMan is unavailable.' });
          }
        })();
        return true;

      case 'youtube-verification-request':
      case 'youtube-verification-details':
      case 'youtube-verification-approve':
      case 'youtube-verification-cancel':
        void handleYoutubeVerificationMessage(msg, sender).then(sendResponse).catch(() => {
          sendResponse({ success: false, error: 'Could not share the YouTube session. Keep the original video tab open, complete verification, and retry. If this request expired, start again in DLMan.' });
        });
        return true;

      case 'media-detected':
        // Content script detected media on a page — log it
        if (msg.media) {
          console.log(
            '[DLMan] Media detected:',
            msg.media.protocol,
            msg.media.variants?.length ?? 0,
            'variants',
          );
        }
        sendResponse({ success: true });
        return true;

      case 'media-download':
        // Content script requests downloading detected media
        (async () => {
          if (!msg.request) {
            sendResponse({ success: false, error: 'No download request provided' });
            return;
          }
          try {
            const client = getDlmanClient();
            const isAvailable = await client.ping();
            if (!isAvailable) {
              // Fallback: for direct media, try opening as a regular download dialog
              if (msg.request.media.protocol === 'direct') {
                await handleDownload(
                  msg.request.media.master_url,
                  msg.request.media.referrer,
                  msg.request.media.filename || undefined,
                );
                sendResponse({ success: true });
              } else {
                sendResponse({ success: false, error: 'DLMan is not running' });
              }
              return;
            }

            // Webpage selections stay separate from raw direct/HLS/DASH protocols.
            if (msg.request.media.protocol === 'webpage' && (!msg.request.inspection_id || !msg.request.web_media_selection)) {
              sendResponse({ success: false, error: 'Select a video quality first.' }); return;
            }
            const result = msg.request.media.protocol === 'webpage'
              ? await client.downloadWebMedia({ url: msg.request.media.extraction_url || msg.request.media.page_url,
                  inspection_id: msg.request.inspection_id!, selection: msg.request.web_media_selection! })
              : await client.downloadMedia(msg.request);
            sendResponse({
              success: result.success,
              error: result.error,
            });
          } catch (error) {
            sendResponse({ success: false, error: (error as Error).message });
          }
        })();
        return true;

      default:
        return true;
    }
  });

  // Initialize
  init();
});
