import { useEffect, useState } from 'react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { useDownloadRefresh } from '@/stores/download-refresh';
import { useDownloadStore } from '@/stores/downloads';
import { formatBytes } from '@/lib/utils';
import { refreshSessionExpired } from '../../../../../shared/download-refresh.mjs';

export function RefreshDownloadAddressDialog() {
  const { t } = useTranslation();
  const { download, session, busy, error, close, verify, releaseCandidate } = useDownloadRefresh();
  const exists = useDownloadStore(state => download ? state.downloads.has(download.id) : true);
  const [url, setUrl] = useState('');

  useEffect(() => { setUrl(''); }, [download?.id]);
  useEffect(() => {
    if (!exists && download) void close();
  }, [exists, download, close]);
  useEffect(() => {
    if (!session) return;
    const check = () => {
      if (refreshSessionExpired(session)) {
        toast.info(t('refreshAddress.expired'));
        void close();
      }
    };
    const timer = window.setInterval(check, 1000);
    check();
    return () => window.clearInterval(timer);
  }, [session, close, t]);

  return (
    <Dialog open={download !== null} onOpenChange={open => { if (!open) void close(); }}>
      <DialogContent className="sm:max-w-[540px]" onInteractOutside={event => event.preventDefault()}>
        <DialogHeader>
          <DialogTitle>{t('refreshAddress.title')}</DialogTitle>
          <DialogDescription>{t('refreshAddress.description')}</DialogDescription>
        </DialogHeader>
        {download && <div className="space-y-4 min-w-0">
          <div className="rounded-md bg-muted p-3 text-sm">
            <p className="font-medium break-all">{download.filename}</p>
            <p className="text-muted-foreground">{t('refreshAddress.saved', { size: formatBytes(download.downloaded) })}</p>
          </div>
          <p className="text-sm text-muted-foreground">{download.source_page_url ? t('refreshAddress.browserInstructions') : t('refreshAddress.noSourcePage')}</p>
          <Button variant="outline" disabled={!session || !download.source_page_url || busy} onClick={() => {
            const source = download.source_page_url;
            if (!source) return;
            try {
              const parsed = new URL(source);
              if (!['http:', 'https:'].includes(parsed.protocol)) return;
              void openUrl(source).catch(() => toast.error(t('refreshAddress.openFailed')));
            } catch { toast.error(t('refreshAddress.openFailed')); }
          }}>{t('refreshAddress.openSourcePage')}</Button>
          {session?.candidate && <div className="rounded-md border p-3 space-y-3">
            <p className="text-sm font-medium">{t('refreshAddress.candidate')}</p>
            {session.candidate.filename && <p className="text-sm break-all">{session.candidate.filename}</p>}
            <p className="text-xs font-mono break-all">{session.candidate.display_url}</p>
            <div className="flex flex-wrap gap-2">
              <Button disabled={busy} onClick={() => void verify(undefined, session.candidate!.candidate_id)}>{t('refreshAddress.useLink')}</Button>
              <Button variant="outline" disabled={busy} onClick={() => void releaseCandidate()}>{t('refreshAddress.addAsNew')}</Button>
            </div>
          </div>}
          <div className="space-y-2">
            <Label htmlFor="replacement-download-url">{t('refreshAddress.replacementUrl')}</Label>
            <Input id="replacement-download-url" type="url" value={url} onChange={event => setUrl(event.target.value)}
              placeholder="https://" disabled={!session || busy} autoComplete="off" spellCheck={false}
              onKeyDown={event => { if (event.key === 'Enter' && url.trim()) void verify(url); }} />
          </div>
          <p className="text-xs text-muted-foreground">{busy ? t('refreshAddress.verifying') : t('refreshAddress.waiting')}</p>
          {error && <p role="alert" className="text-sm text-destructive break-words">{error}</p>}
        </div>}
        <DialogFooter>
          <Button variant="outline" onClick={() => void close()}>{t('common.cancel')}</Button>
          <Button disabled={!session || busy || !url.trim()} onClick={() => void verify(url)}>{t('refreshAddress.verifyResume')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
