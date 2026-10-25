import { useYoutubeInspection } from '@/hooks/useYoutubeInspection';
import { useWebMediaInspection } from '@/hooks/useWebMediaInspection';
import { webMediaFilename } from '../../../../../shared/web-media.mjs';
import { webMediaExtensionContext } from '@/lib/web-media-context';
import { isYoutubeHost, type YoutubeSelection } from '../../../../../shared/youtube.mjs';
import { useState, useCallback, useEffect, useRef, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { motion, AnimatePresence } from 'framer-motion';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import {
  Link, 
  Folder, 
  Download, 
  Loader2, 
  CheckCircle2,
  XCircle,
  Clipboard,
  Tag,
  Save,
  Clock,
  FileText,
  ChevronDown,
  ChevronUp,
  KeyRound,
  ShieldAlert,
} from 'lucide-react';

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { useUIStore } from '@/stores/ui';
import { useQueuesArray, useQueueStore } from '@/stores/queues';
import { useDownloadStore } from '@/stores/downloads';
import { useCategoryStore } from '@/stores/categories';
import { getPendingClipboardUrls, getPendingDropUrls, getPendingCookies, getPendingMediaMeta, getPendingSourcePage } from '@/lib/events';
import { getDefaultBasePath, getCategoryDownloadPath, detectCategoryFromFilename } from '@/lib/download-path';
import type { LinkInfo, Download as DownloadType } from '@/types';

// Check if we're in Tauri context
const isTauri = () => typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__ !== undefined;

export function NewDownloadDialog() {
  const { t } = useTranslation();
  const { showNewDownloadDialog, setShowNewDownloadDialog } = useUIStore();
  // Bumped whenever URLs are routed here (drop/paste/extension). We re-consume
  // on every change so a link dropped while the dialog is already open still
  // populates the field instead of being ignored.
  const urlIntakeNonce = useUIStore((s) => s.urlIntakeNonce);
  const queues = useQueuesArray();
  const selectedQueueId = useQueueStore((s) => s.selectedQueueId);
  const setSelectedQueue = useQueueStore((s) => s.setSelectedQueue);
  const categories = useMemo(
    () => useCategoryStore.getState().categories,
    []
  );
  const updateCategory = useCategoryStore((s) => s.updateCategory);
  const setSelectedCategory = useCategoryStore((s) => s.setSelectedCategory);
  const selectedCategoryId = useCategoryStore((s) => s.selectedCategoryId);
  const addDownload = useDownloadStore((s) => s.addDownload);
  const removeDownload = useDownloadStore((s) => s.removeDownload);
  const setFilter = useDownloadStore((s) => s.setFilter);

  // Default queue UUID (Main queue)
  const DEFAULT_QUEUE_ID = '00000000-0000-0000-0000-000000000000';
  
  const [url, setUrl] = useState('');
  const [destination, setDestination] = useState('');
  const [queueId, setQueueId] = useState(DEFAULT_QUEUE_ID);
  const [categoryId, setCategoryId] = useState<string | null>(null);
  const [filename, setFilename] = useState('');
  const [customFilename, setCustomFilename] = useState(''); // User-edited filename
  const [filenameEdited, setFilenameEdited] = useState(false); // Track if user edited filename
  const [showAdvancedPath, setShowAdvancedPath] = useState(false); // Show separate filename field
  const [fileSize, setFileSize] = useState<number | null>(null);
  const [isProbing, setIsProbing] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [requiresAuth, setRequiresAuth] = useState(false);
  const [isAdding, setIsAdding] = useState(false); // Kept for button disabled state during brief window
  const [rememberPathForCategory, setRememberPathForCategory] = useState(false);
  const [pathCustomized, setPathCustomized] = useState(false);
  const categoryCustomizedRef = useRef(false);
  const webMediaNamedRef = useRef(false);
  // Track if user has manually customized the path (ref for logic, state for UI reactivity)
  const pathCustomizedRef = useRef(false);
  // Trigger to force re-probe when dialog opens (even with same URL)
  const [probeTrigger, setProbeTrigger] = useState(0);
  // Browser cookies passed from extension for session-based auth
  const [sourcePageUrl, setSourcePageUrl] = useState<string | undefined>(undefined);
  const [browserCookies, setBrowserCookies] = useState<string | undefined>(undefined);
  const [browserIntakeUrl, setBrowserIntakeUrl] = useState<string | undefined>(undefined);
  // Media metadata for HLS/DASH streaming downloads (from browser extension)
  const [mediaMeta, setMediaMeta] = useState<{
    media_quality?: import('../../../../../shared/web-media.mjs').WebMediaQualityHint;
    web_media_selection?: import('../../../../../shared/web-media.mjs').WebMediaSelection;
    protocol: string;
    master_url: string;
    page_title?: string;
    variant_index?: number;
    youtube_selection?: YoutubeSelection;
    referrer?: string;
  } | undefined>(undefined);

  const youtube = useYoutubeInspection(url, showNewDownloadDialog, mediaMeta?.youtube_selection);
  const youtubeUnavailable = youtube.enabled && (youtube.loading || !!youtube.error || !youtube.selection);
  const webMediaContext = useMemo(() => webMediaExtensionContext(url, browserIntakeUrl,
    { media: mediaMeta, sourcePageUrl, cookies: browserCookies }),
  [url, browserIntakeUrl, mediaMeta, sourcePageUrl, browserCookies]);
  const webMedia = useWebMediaInspection(url, showNewDownloadDialog, webMediaContext, mediaMeta?.web_media_selection);
  const webMediaUnavailable = webMedia.enabled && (webMedia.loading || !!webMedia.error
    || (!webMedia.file && !webMedia.selection) || webMedia.capture.waiting
    || (!!webMedia.capture.capture && !webMedia.capture.sourceId && webMedia.capture.capture.status === 'ready'));
  const submissionGeneration = useRef(0);
  const submissionContext = useRef('');
  const submissionKey = JSON.stringify([url, showNewDownloadDialog, urlIntakeNonce,
    webMedia.selection, webMedia.capture.sourceId]);
  submissionContext.current = submissionKey;
  useEffect(() => {
    submissionGeneration.current++;
    setIsAdding(false);
  }, [submissionKey]);

  // Tracks which intake we've already processed. Keyed on the intake nonce so
  // the effect runs once per open AND once per new drop/paste while open, but
  // not on unrelated re-renders (which would clobber consume-once pending reads
  // under React StrictMode).
  const processedIntakeRef = useRef<string>('');

  // Reset state and check for pending URLs when the dialog opens or a new URL
  // is routed in while it's already open.
  useEffect(() => {
    if (!showNewDownloadDialog) {
      // Dialog closing — arm the next open to re-process.
      processedIntakeRef.current = '';
      return;
    }

    const intakeKey = String(urlIntakeNonce);
    if (processedIntakeRef.current === intakeKey) return;
    processedIntakeRef.current = intakeKey;

    // Check for pending clipboard/drop URLs
    const clipboardUrls = getPendingClipboardUrls();
    const dropUrls = getPendingDropUrls();
    const pendingUrls = clipboardUrls.length > 0 ? clipboardUrls : dropUrls;
    
    // Check for pending cookies from browser extension
    const cookies = getPendingCookies();
    setBrowserCookies(cookies);
    setBrowserIntakeUrl(pendingUrls[0]);
    setSourcePageUrl(getPendingSourcePage());
    
    // Check for media metadata (HLS/DASH streaming)
    const meta = getPendingMediaMeta();
    setMediaMeta(meta);
    
    if (pendingUrls.length > 0) {
      setUrl(pendingUrls[0]);
    } else {
      setUrl('');
    }
    
    setFilename('');
    setCustomFilename('');
    setFilenameEdited(false);
    categoryCustomizedRef.current = false;
    webMediaNamedRef.current = false;
    setShowAdvancedPath(false);
    setFileSize(null);
    setProbeError(null);
    setCategoryId(null);
    setRememberPathForCategory(false);
    setPathCustomized(false);
    pathCustomizedRef.current = false;
    // Force re-probe even if URL is the same as before
    setProbeTrigger(prev => prev + 1);
    
    // Set queue to selected queue if viewing a queue, otherwise use Main queue
    setQueueId(selectedQueueId ?? DEFAULT_QUEUE_ID);
    
    // Set default path
    initializeDefaultPath();
  }, [showNewDownloadDialog, selectedQueueId, urlIntakeNonce]);

  const initializeDefaultPath = async () => {
    const basePath = await getDefaultBasePath();
    setDestination(basePath);
  };

  // Update category and path when filename changes (auto-detect)
  const updateCategoryFromFilename = useCallback(async (newFilename: string) => {
    if (categoryCustomizedRef.current) return;
    const detectedCategory = detectCategoryFromFilename(newFilename);
    if (detectedCategory) {
      setCategoryId(detectedCategory.id);
      // Only update path if user hasn't customized it
      if (!pathCustomizedRef.current) {
        const newPath = await getCategoryDownloadPath(detectedCategory.id);
        setDestination(newPath);
      }
    }
  }, []);

  // Handle manual category change
  const handleCategoryChange = async (newCategoryId: string) => {
    categoryCustomizedRef.current = true;
    const id = newCategoryId === 'none' ? null : newCategoryId;
    setCategoryId(id);
    // Only update path if user hasn't customized it
    if (!pathCustomizedRef.current) {
      const newPath = await getCategoryDownloadPath(id);
      setDestination(newPath);
    }
  };

  // Handle manual path change (marks as customized)
  const handleDestinationChange = (newPath: string) => {
    pathCustomizedRef.current = true;
    setPathCustomized(true);
    setDestination(newPath);
  };

  // Handle custom filename change
  const handleFilenameChange = (newFilename: string) => {
    setCustomFilename(newFilename);
    setFilenameEdited(true);
  };

  // Get the effective filename to use (custom if edited, otherwise probed)
  const effectiveFilename = filenameEdited && customFilename ? customFilename : filename;

  // Standalone t() (not nested in another t()'s args) so i18next-parser sees it.
  const thisCategoryLabel = t('newDownload.thisCategory');

  // Probe URL when it changes (debounced)
  useEffect(() => {
    if (!showNewDownloadDialog) {
      return;
    }

    if (isYoutubeHost(url)) { setIsProbing(false); setRequiresAuth(false); setProbeError(null); return; }
    // Generic HTTP(S) links are classified before ordinary probing. Video pages
    // and manifests never enter the file transfer flow on extraction failure.
    if (webMedia.enabled && !webMedia.file) {
      setIsProbing(false); setRequiresAuth(false); setProbeError(null);
      setFilename(''); setFileSize(null);
      return;
    }

    if (!url) {
      setFilename('');
      setFileSize(null);
      setProbeError(null);
      setRequiresAuth(false);
      return;
    }

    let cancelled = false;

    const timer = setTimeout(async () => {
      try {
        if (cancelled) return;
        setIsProbing(true);
        setProbeError(null);
        setRequiresAuth(false);

        const results = await invoke<LinkInfo[]>('probe_links', { urls: [url] });
        if (cancelled) return;
        const info = results[0];

        if (info?.error) {
          setProbeError(info.error);
          setFilename('');
          setFileSize(null);
          setRequiresAuth(false);
        } else if (info) {
          // For HLS/DASH streams, the probed filename is the manifest name
          // (e.g. "master.m3u8") which is useless. Use page_title instead.
          let probedName = info.filename;
          const lower = probedName.toLowerCase();
          if (lower.endsWith('.m3u8') || lower.endsWith('.mpd') ||
              lower === 'master' || lower === 'index' || lower === 'playlist') {
            // Try to use page_title from media metadata
            if (mediaMeta?.page_title) {
              const sanitized = mediaMeta.page_title.replace(/[\/\\:*?"<>|]/g, '_').trim();
              if (sanitized.length > 0 && sanitized.length <= 200) {
                probedName = sanitized + '.ts';
              } else {
                probedName = 'video.ts';
              }
            } else {
              probedName = 'video.ts';
            }
          }
          setFilename(probedName);
          // Only set custom filename if user hasn't edited it
          if (!filenameEdited) {
            setCustomFilename(probedName);
          }
          setFileSize(info.size ?? null);
          setProbeError(null);
          setRequiresAuth(info.requires_auth ?? false);
          // Auto-detect category from filename
          updateCategoryFromFilename(probedName);
        }
      } catch (err) {
        if (cancelled) return;
        setProbeError(err instanceof Error ? err.message : t('newDownload.probeFailed'));
      } finally {
        if (cancelled) return;
        setIsProbing(false);
      }
    }, 500);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [url, probeTrigger, updateCategoryFromFilename, showNewDownloadDialog, mediaMeta, t, webMedia.enabled, webMedia.file]);

  const handlePasteFromClipboard = useCallback(async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text && (text.startsWith('http://') || text.startsWith('https://'))) {
        setMediaMeta(undefined); setBrowserCookies(undefined); setSourcePageUrl(undefined);
        setUrl(text);
      }
    } catch (err) {
      console.error('Failed to read clipboard:', err);
    }
  }, []);

  const handleBrowseDestination = useCallback(async () => {
    // Check if we're in Tauri context
    if (!isTauri()) {
      toast.error(t('toasts.browseDesktopOnly'));
      return;
    }

    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: destination.startsWith('~')
          ? undefined // Let Tauri use default
          : destination,
      });

      if (selected) {
        pathCustomizedRef.current = true;
        setPathCustomized(true);
        setDestination(selected as string);
      }
    } catch (err) {
      console.error('Failed to open directory picker:', err);
      toast.error(t('toasts.dirPickerFailed'));
    }
  }, [destination, t]);

  useEffect(() => {
    if (!youtube.preview || !youtube.selection) return;
    const extension = youtube.selection.container;
    const title = youtube.preview.title.replace(/[\/\\:*?"<>|]/g, '_').trim().slice(0, 160) || 'YouTube video';
    const name = `${title}.${extension}`;
    setFilename(name);
    setFileSize(youtube.preview.variants.find((v) => v.id === youtube.selection?.format_id && v.output_mode === youtube.selection?.output_mode)?.size ?? null);
    if (!filenameEdited) setCustomFilename(name);
    updateCategoryFromFilename(name);
  }, [youtube.preview, youtube.selection, filenameEdited, updateCategoryFromFilename]);

  useEffect(() => {
    if (!webMedia.preview || !webMedia.selection) return;
    const name = webMediaFilename(webMedia.preview.title, webMedia.selection.container);
    setFilename(name);
    setFileSize(webMedia.preview.variants.find((variant) => variant.id === webMedia.selection?.format_id)?.size ?? null);
    setCustomFilename((previous) => filenameEdited
      ? webMediaFilename(webMedia.preview!.title, webMedia.selection!.container, previous) : name);
    if (!webMediaNamedRef.current) {
      webMediaNamedRef.current = true;
      if (!webMedia.capture.capture && !webMedia.capture.waiting) void updateCategoryFromFilename(name);
    }
  }, [webMedia.preview, webMedia.selection, webMedia.capture.capture, webMedia.capture.waiting, filenameEdited, updateCategoryFromFilename]);

  const handleAddDownload = useCallback(async (startLater: boolean = false) => {
    if (!url || !destination || youtubeUnavailable || webMediaUnavailable) {
      toast.error(t('toasts.enterUrlAndDest'));
      return;
    }
    const submission = ++submissionGeneration.current;
    const isCurrentSubmission = () => submission === submissionGeneration.current && submissionContext.current === submissionKey;

    let refreshedInspectionId = youtube.inspectionId;
    if (youtube.enabled) {
      setIsAdding(true);
      try { refreshedInspectionId = await youtube.refreshSelection(); }
      catch (error) { if (isCurrentSubmission()) toast.error(error instanceof Error ? error.message : String(error)); return; }
      finally { if (isCurrentSubmission()) setIsAdding(false); }
    }
    let webInspectionId = webMedia.inspectionId;
    if (webMedia.enabled && !webMedia.file) {
      setIsAdding(true);
      try { webInspectionId = await webMedia.refreshSelection(); }
      catch (error) { if (isCurrentSubmission()) toast.error(error instanceof Error ? error.message : String(error)); return; }
      finally { if (isCurrentSubmission()) setIsAdding(false); }
    }
    if (!isCurrentSubmission()) return;

    // Get the filename to use (custom if edited, otherwise probed)
    const requestedFilename = filenameEdited && customFilename ? customFilename : filename;
    const filenameToUse = webMedia.selection && webMedia.preview
      ? webMediaFilename(webMedia.preview.title, webMedia.selection.container, requestedFilename) : requestedFilename;
    
    // OPTIMISTIC UI: Close dialog immediately and add placeholder
    const tempId = crypto.randomUUID();
    const optimisticDownload: DownloadType = {
      id: tempId,
      url,
      final_url: null,
      filename: filenameToUse || url.split('/').pop() || 'downloading...',
      destination,
      size: fileSize,
      downloaded: 0,
      status: startLater ? 'queued' : 'pending',
      segments: [],
      queue_id: queueId,
      category_id: categoryId,
      color: null,
      error: null,
      speed_limit: null,
      created_at: new Date().toISOString(),
      completed_at: null,
    };
    
    // Add optimistically and close dialog immediately for snappy UX
    addDownload(optimisticDownload);
    setShowNewDownloadDialog(false);
    
    // Show immediate feedback
    if (startLater) {
      toast.success(t('toasts.downloadAddedToQueue'));
    } else {
      toast.success(t('toasts.downloadStarting'));
    }
    
    // If user chose to remember the path for the category, update now
    if (rememberPathForCategory && categoryId) {
      updateCategory(categoryId, { customPath: destination });
    }
    
    // Navigate to the download's view so user can see it
    // 1. Reset filter to "all" so the new download is visible
    setFilter('all');
    
    // 2. Navigate to the destination queue (or "All Downloads" if using default)
    if (selectedQueueId !== null && selectedQueueId !== queueId) {
      setSelectedQueue(queueId);
    }
    
    // 3. Auto-switch to the download's category view (only if user was in a specific category)
    if (categoryId && selectedCategoryId !== null && selectedCategoryId !== categoryId) {
      setSelectedCategory(categoryId);
    }

    // Now perform backend add in background
    // The backend will emit events (DownloadAdded, DownloadStatusChanged) that update the store
    // We just need to swap our temp ID for the real one
    if (isTauri()) {
      try {
        if (youtube.enabled && youtube.selection && youtube.inspectionId) {
          await invoke<DownloadType>('start_youtube_download', {
            url, inspection_id: refreshedInspectionId, selection: youtube.selection,
            destination, queue_id: queueId, category_id: categoryId || null,
            filename: filenameToUse || null, start_later: startLater,
          });
        } else if (webMedia.enabled && webMedia.selection && webInspectionId) {
          await invoke<DownloadType>('start_web_media_download', {
            url: webMedia.transferUrl, inspection_id: webInspectionId, selection: webMedia.selection,
            destination, queue_id: queueId, category_id: categoryId || null,
            filename: filenameToUse || null, start_later: startLater,
          });
        } else {
          // Regular download — standard flow
          const probedInfo = (filenameToUse || fileSize) ? {
            filename: filenameToUse || undefined,
            size: fileSize ?? undefined,
            final_url: undefined,
          } : undefined;

          await invoke<DownloadType>('add_download', {
            url,
            destination,
            queue_id: queueId,
            category_id: categoryId || undefined,
            probed_info: probedInfo,
            start_later: startLater,
            cookies: browserCookies || undefined,
            source_page_url: sourcePageUrl || undefined,
          });
        }
        
        // Remove our optimistic placeholder - the real download is added via DownloadAdded event
        // which also properly tracks status changes from the backend
        removeDownload(tempId);
        // DON'T add the returned download here - let events handle it
        // The backend emits DownloadAdded which adds it, then DownloadStatusChanged updates status
      } catch (err) {
        console.error('Backend add_download failed:', err);
        // Remove the optimistic download on error
        removeDownload(tempId);
        // Show error with details
        const errorMsg = err instanceof Error ? err.message : String(err);
        toast.error(t('toasts.addDownloadFailed'), { description: errorMsg });
      }
    }
  }, [url, destination, queueId, categoryId, filename, customFilename, filenameEdited, fileSize, browserCookies, sourcePageUrl, mediaMeta, youtube, youtubeUnavailable, webMedia, webMediaUnavailable, submissionKey, addDownload, removeDownload, setShowNewDownloadDialog, rememberPathForCategory, updateCategory, selectedCategoryId, setSelectedCategory, setFilter, setSelectedQueue, selectedQueueId, t]);

  const formatFileSize = (bytes: number) => {
    if (bytes >= 1024 * 1024 * 1024) {
      return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
    }
    if (bytes >= 1024 * 1024) {
      return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
    }
    if (bytes >= 1024) {
      return `${(bytes / 1024).toFixed(2)} KB`;
    }
    return `${bytes} B`;
  };

  return (
    <Dialog open={showNewDownloadDialog} onOpenChange={setShowNewDownloadDialog}>
      <DialogContent className="sm:max-w-[540px] max-h-[88vh] overflow-hidden flex flex-col gap-0 p-0">
        <DialogHeader className="shrink-0 flex-row items-center gap-3 space-y-0 border-b px-6 py-4">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary ring-1 ring-inset ring-primary/20">
            <Download className="h-5 w-5" />
          </div>
          <div className="min-w-0 flex-1 text-left">
            <DialogTitle className="text-base">{t('menu.newDownload')}</DialogTitle>
            <DialogDescription className="mt-0.5 text-xs">
              {t('newDownload.desc')}
            </DialogDescription>
          </div>
        </DialogHeader>

        <div className="flex-1 min-h-0 overflow-y-auto">
          <div className="space-y-5 px-6 py-5">
          {/* URL Input */}
          <div className="space-y-2">
            <Label htmlFor="url" className="flex items-center gap-2 text-xs font-medium text-muted-foreground">
              <Link className="h-3.5 w-3.5" />
              {t('newDownload.url')}
            </Label>
            <div className="flex gap-2">
              <div className="relative flex-1">
                <Input
                  id="url"
                  value={url}
                  onChange={(e) => { setMediaMeta(undefined); setBrowserCookies(undefined); setSourcePageUrl(undefined); setUrl(e.target.value); }}
                  placeholder={t('newDownload.urlPlaceholder')}
                  className="h-11 pr-10"
                />
                <AnimatePresence>
                  {isProbing && (
                    <motion.div
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      exit={{ opacity: 0 }}
                      className="absolute right-3 inset-y-0 flex items-center"
                    >
                      <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
                    </motion.div>
                  )}
                  {!isProbing && filename && !probeError && !requiresAuth && (
                    <motion.div
                      initial={{ opacity: 0, scale: 0.8 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.8 }}
                      className="absolute right-3 inset-y-0 flex items-center"
                    >
                      <CheckCircle2 className="h-4 w-4 text-green-500" />
                    </motion.div>
                  )}
                  {!isProbing && requiresAuth && !probeError && (
                    <motion.div
                      initial={{ opacity: 0, scale: 0.8 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.8 }}
                      className="absolute right-3 inset-y-0 flex items-center"
                    >
                      <ShieldAlert className="h-4 w-4 text-amber-500" />
                    </motion.div>
                  )}
                  {!isProbing && probeError && (
                    <motion.div
                      initial={{ opacity: 0, scale: 0.8 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.8 }}
                      className="absolute right-3 inset-y-0 flex items-center"
                    >
                      <XCircle className="h-4 w-4 text-destructive" />
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
              <Button
                variant="outline"
                size="icon"
                className="h-11 w-11 shrink-0"
                onClick={handlePasteFromClipboard}
                title={t('newDownload.pasteClipboard')}
              >
                <Clipboard className="h-4 w-4" />
              </Button>
            </div>
            {probeError && (
              <p className="text-sm text-destructive">{probeError}</p>
            )}
          </div>

          {/* Authentication Required Warning */}
          <AnimatePresence>
            {requiresAuth && !probeError && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="rounded-md border border-amber-500/30 bg-amber-500/5 p-3"
              >
                <div className="flex items-center gap-3">
                  <ShieldAlert className="h-5 w-5 text-amber-500 flex-shrink-0" />
                  <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium text-amber-600 dark:text-amber-400">
                      {t('newDownload.authRequired')}
                    </p>
                    <p className="text-xs text-muted-foreground mt-0.5">
                      {t('newDownload.authHint')}
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="flex-shrink-0 border-amber-500/30 text-amber-600 hover:bg-amber-500/10 dark:text-amber-400"
                    onClick={() => {
                      useUIStore.getState().setShowSettingsDialog(true);
                    }}
                  >
                    <KeyRound className="h-3.5 w-3.5 mr-1.5" />
                    {t('newDownload.savedLogins')}
                  </Button>
                </div>
              </motion.div>
            )}
          </AnimatePresence>

          {/* File Info - Shows detected file name and size */}
          <AnimatePresence>
            {filename && !probeError && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="rounded-xl border bg-muted/40 p-3 space-y-3"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2.5 flex-1 min-w-0">
                    <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-background text-muted-foreground ring-1 ring-inset ring-border">
                      <FileText className="h-4 w-4" />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="text-sm font-medium truncate">{effectiveFilename}</span>
                        {filenameEdited && (
                          <span className="shrink-0 text-[10px] text-primary bg-primary/10 px-1.5 py-0.5 rounded">{t('newDownload.customBadge')}</span>
                        )}
                      </div>
                      <span className="text-xs text-muted-foreground">
                        {fileSize ? formatFileSize(fileSize) : (isProbing ? t('newDownload.adding') : '—')}
                      </span>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 flex-shrink-0">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setShowAdvancedPath(!showAdvancedPath)}
                      className="h-7 px-2 text-xs"
                    >
                      {showAdvancedPath ? (
                        <>
                          <ChevronUp className="h-3 w-3 mr-1" />
                          {t('newDownload.less')}
                        </>
                      ) : (
                        <>
                          <ChevronDown className="h-3 w-3 mr-1" />
                          {t('newDownload.rename')}
                        </>
                      )}
                    </Button>
                  </div>
                </div>
                
                {/* Expanded filename editing */}
                <AnimatePresence>
                  {showAdvancedPath && (
                    <motion.div
                      initial={{ opacity: 0, height: 0 }}
                      animate={{ opacity: 1, height: 'auto' }}
                      exit={{ opacity: 0, height: 0 }}
                      className="space-y-2"
                    >
                      <Label htmlFor="custom-filename" className="text-xs text-muted-foreground">
                        {t('newDownload.fileName')}
                      </Label>
                      <Input
                        id="custom-filename"
                        value={customFilename}
                        onChange={(e) => handleFilenameChange(e.target.value)}
                        placeholder={t('newDownload.filenamePlaceholder')}
                        className="h-8 text-sm"
                      />
                      {filenameEdited && customFilename !== filename && (
                        <div className="flex items-center gap-2">
                          <span className="text-xs text-muted-foreground">{t('newDownload.original', { name: filename })}</span>
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => {
                              setCustomFilename(filename);
                              setFilenameEdited(false);
                            }}
                            className="h-5 px-1.5 text-xs"
                          >
                            {t('newDownload.reset')}
                          </Button>
                        </div>
                      )}
                    </motion.div>
                  )}
                </AnimatePresence>
              </motion.div>
            )}
          </AnimatePresence>

          {youtube.enabled && (
            <div className="space-y-2">
              <Label className="text-xs font-medium text-muted-foreground">{t('youtube.quality', 'Quality')}</Label>
              {youtube.loading ? <div className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t('youtube.loading', 'Loading video formats…')}</div>
                : youtube.error ? <div className="space-y-2">
                    <p className="text-sm text-destructive" role="alert">{youtube.error}</p>
                    <div className="flex flex-wrap gap-2">
                      <Button size="sm" variant="outline" disabled={youtube.verification.waiting} onClick={youtube.retry}>{t('youtube.retry', 'Retry')}</Button>
                      <Button size="sm" variant="outline" disabled={youtube.verification.waiting} onClick={() => { void youtube.verification.verify(); }}>{t('youtube.verify', 'Verify in browser')}</Button>
                      {youtube.verification.waiting && <Button size="sm" variant="ghost" onClick={youtube.verification.cancel}>{t('common.cancel', 'Cancel')}</Button>}
                    </div>
                    {youtube.verification.waiting && <p className="text-xs text-muted-foreground" role="status">{t('youtube.verifying', 'Enable the DLMan extension in the browser that opened. Complete verification on YouTube, then click Share session in the extension approval window. Waiting up to 10 minutes…')}</p>}
                    {youtube.verification.error && <p className="text-sm text-destructive" role="alert">{youtube.verification.error}</p>}
                  </div>
                : <Select value={youtube.selection ? `${youtube.selection.output_mode}:${youtube.selection.format_id}` : ''} onValueChange={youtube.choose}>
                    <SelectTrigger><SelectValue placeholder={t('youtube.select', 'Select quality')} /></SelectTrigger>
                    <SelectContent>{youtube.preview?.variants.map((variant) => <SelectItem key={`${variant.output_mode}:${variant.id}`} value={`${variant.output_mode}:${variant.id}`}>
                      {variant.output_mode === 'mp3' ? variant.quality : `${variant.quality} · ${variant.container.toUpperCase()}`}
                    </SelectItem>)}</SelectContent>
                  </Select>}
            </div>
          )}

          {webMedia.enabled && (!webMedia.file || webMedia.capture.capture || webMedia.capture.waiting || webMedia.capture.error) && (
            <div className="space-y-2">
              <Label className="text-xs font-medium text-muted-foreground">{t('webMedia.quality', 'Video quality')}</Label>
              {webMedia.loading && <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status"><Loader2 className="h-4 w-4 animate-spin" />{t('webMedia.loading', 'Checking for downloadable video…')}</div>}
              {webMedia.error && <p className="text-sm text-destructive" role="alert">{webMedia.error}</p>}
              {webMedia.preview && webMedia.selection && (
                <Select value={webMedia.selection.format_id} onValueChange={webMedia.choose} disabled={webMedia.capture.waiting}>
                  <SelectTrigger><SelectValue placeholder={t('webMedia.selectQuality', 'Select quality')} /></SelectTrigger>
                  <SelectContent>{webMedia.preview.variants.map((variant) => <SelectItem key={variant.id} value={variant.id}>
                    {variant.quality} · {variant.container.toUpperCase()}{variant.fps ? ` · ${variant.fps} fps` : ''}{variant.audio ? ` · ${variant.audio}` : ''}
                  </SelectItem>)}</SelectContent>
                </Select>
              )}
              {webMedia.capture.capture?.status === 'ready' && webMedia.capture.capture.sources.length > 1 && (
                <div className="space-y-2">
                  <Label>{t('webMedia.source', 'Detected video')}</Label>
                  <Select value={webMedia.capture.sourceId || ''} onValueChange={webMedia.capture.chooseSource}>
                    <SelectTrigger><SelectValue placeholder={t('webMedia.selectSource', 'Select a detected video')} /></SelectTrigger>
                    <SelectContent>{webMedia.capture.capture.sources.map((source, index) => <SelectItem key={source.id} value={source.id}>
                      {source.title || `${t('webMedia.video', 'Video')} ${index + 1}`} · {source.protocol.toUpperCase()}
                    </SelectItem>)}</SelectContent>
                  </Select>
                </div>
              )}
              <div className="flex flex-wrap gap-2">
                <Button size="sm" variant="outline" disabled={webMedia.loading || webMedia.capture.waiting} onClick={webMedia.retry}>{t('webMedia.retry', 'Retry')}</Button>
                <Button size="sm" variant="outline" disabled={webMedia.capture.waiting && !webMedia.capture.extensionMissing} onClick={() => { void webMedia.capture.begin(); }}>{t('webMedia.detect', 'Detect in browser')}</Button>
                {webMedia.capture.waiting && <Button size="sm" variant="ghost" onClick={webMedia.capture.cancel}>{t('common.cancel', 'Cancel')}</Button>}
              </div>
              {webMedia.capture.waiting && <p className="text-xs text-muted-foreground" role="status">
                {webMedia.capture.capture?.status === 'watching'
                  ? t('webMedia.watching', 'Browser connected. Play the video on the opened page. Waiting up to 10 minutes…')
                  : t('webMedia.waiting', 'Connecting to the DLMan extension. Enable it in the opened browser, then play the video. Waiting up to 10 minutes…')}
              </p>}
              {webMedia.capture.extensionMissing && <p className="text-xs text-muted-foreground" role="status">{t('webMedia.extensionMissing', 'The DLMan extension has not connected. Install or enable it in the browser that opened, then retry Detect in browser.')}</p>}
              {webMedia.capture.error && <p className="text-sm text-destructive" role="alert">{webMedia.capture.error}</p>}
            </div>
          )}

          {/* Destination */}
          <div className="space-y-2">
            <Label htmlFor="destination" className="flex items-center gap-2 text-xs font-medium text-muted-foreground">
              <Folder className="h-3.5 w-3.5" />
              {t('newDownload.saveTo')}
            </Label>
            <div className="flex gap-2">
              <Input
                id="destination"
                value={destination}
                onChange={(e) => handleDestinationChange(e.target.value)}
                placeholder={t('newDownload.destinationPlaceholder')}
                className="flex-1"
              />
              <Button
                variant="outline"
                onClick={handleBrowseDestination}
              >
                {t('newDownload.browse')}
              </Button>
            </div>
            {/* Remember path for category - only show when path is customized and category is selected */}
            <AnimatePresence>
              {pathCustomized && categoryId && (
                <motion.div
                  initial={{ opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: 'auto' }}
                  exit={{ opacity: 0, height: 0 }}
                  className="flex items-center gap-2 pt-1"
                >
                  <Switch
                    id="remember-path"
                    checked={rememberPathForCategory}
                    onCheckedChange={setRememberPathForCategory}
                  />
                  <Label htmlFor="remember-path" className="flex items-center gap-2 text-sm text-muted-foreground cursor-pointer">
                    <Save className="h-3 w-3" />
                    {t('newDownload.rememberPath', { category: categories.get(categoryId)?.name || thisCategoryLabel })}
                  </Label>
                </motion.div>
              )}
            </AnimatePresence>
          </div>

          {/* Category Selection */}
          <div className="space-y-2">
            <Label htmlFor="category" className="flex items-center gap-2 text-xs font-medium text-muted-foreground">
              <Tag className="h-3.5 w-3.5" />
              {t('newDownload.category')}
              {categoryId && (
                <span className="text-[10px] font-normal text-primary">{t('newDownload.autoDetected')}</span>
              )}
            </Label>
            <Select value={categoryId || 'none'} onValueChange={handleCategoryChange}>
              <SelectTrigger>
                <SelectValue placeholder={t('newDownload.selectCategory')} />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">
                  <div className="flex items-center gap-2 text-muted-foreground">
                    {t('newDownload.noCategory')}
                  </div>
                </SelectItem>
                {Array.from(categories.values()).map((category) => (
                  <SelectItem key={category.id} value={category.id}>
                    <div className="flex items-center gap-2">
                      <div
                        className="h-3 w-3 rounded-full"
                        style={{ backgroundColor: category.color }}
                      />
                      {category.name}
                    </div>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* Queue Selection */}
          <div className="space-y-2">
            <Label htmlFor="queue" className="text-xs font-medium text-muted-foreground">{t('newDownload.queue')}</Label>
            <Select value={queueId} onValueChange={setQueueId}>
              <SelectTrigger>
                <SelectValue placeholder={t('newDownload.selectQueue')} />
              </SelectTrigger>
              <SelectContent>
                {queues.map((queue) => (
                  <SelectItem key={queue.id} value={queue.id}>
                    <div className="flex items-center gap-2">
                      <div
                        className="h-3 w-3 rounded-full"
                        style={{ backgroundColor: queue.color }}
                      />
                      {queue.name}
                    </div>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          </div>
        </div>

        {/* Video pages require a ready inspection and selected quality. Ordinary
            files may start as soon as classification finishes. */}
        <DialogFooter className="shrink-0 flex-row items-center justify-end gap-2 border-t px-6 py-4">
          <Button
            variant="ghost"
            onClick={() => setShowNewDownloadDialog(false)}
          >
            {t('common.cancel')}
          </Button>
          <Button
            variant="outline"
            onClick={() => handleAddDownload(true)}
            disabled={!url || !destination || isAdding || youtubeUnavailable || webMediaUnavailable}
            title={t('newDownload.downloadLaterTitle')}
          >
            <Clock className="mr-2 h-4 w-4" />
            {t('newDownload.downloadLater')}
          </Button>
          <Button
            onClick={() => handleAddDownload(false)}
            disabled={!url || !destination || isAdding || youtubeUnavailable || webMediaUnavailable}
          >
            {isAdding ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                {t('newDownload.adding')}
              </>
            ) : (
              <>
                <Download className="mr-2 h-4 w-4" />
                {t('newDownload.startDownload')}
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
