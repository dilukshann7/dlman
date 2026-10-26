import { Download, ExternalLink, Loader2, RefreshCw } from 'lucide-react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import { useUpdateStore } from '@/stores/update';

/** Used by About; notifications open this same lifecycle instead of opening GitHub. */
export function AppUpdateControls() {
  const { t } = useTranslation();
  const { updateInfo, status, error, failedAction, downloaded, total, canInstall,
    checkForUpdates, downloadUpdate, installUpdate } = useUpdateStore();
  const busy = status === 'checking' || status === 'downloading' || status === 'installing';
  const percent = total && total > 0 ? Math.min(100, Math.round(downloaded / total * 100)) : null;
  const supported = updateInfo?.supported;
  return (
    <div className="flex w-full flex-col items-center gap-3" aria-live="polite">
      <Button variant="ghost" size="sm" className="gap-2" onClick={() => void checkForUpdates()}
        disabled={busy || status === 'ready'}>
        <RefreshCw className={`h-4 w-4 ${status === 'checking' ? 'animate-spin' : ''}`} />
        {status === 'checking' ? t('about.checking') : t('about.checkUpdates')}
      </Button>
      {status === 'up-to-date' && <p className="text-sm text-muted-foreground">{t('about.upToDate')}</p>}
      {updateInfo?.hasUpdate && (
        <>
          <p className="text-sm text-emerald-600 dark:text-emerald-400">
            {t('about.newVersion', { version: updateInfo.latestVersion })}
          </p>
          {supported && (status === 'available' || (status === 'error' && failedAction === 'download')) && (
            <Button size="sm" className="gap-2" onClick={() => void downloadUpdate()}>
              <Download className="h-4 w-4" />
              {status === 'error' ? t('update.retryDownload') : t('update.download')}
            </Button>
          )}
          {status === 'downloading' && (
            <div className="w-full space-y-2">
              <p className="text-center text-sm">{t('update.downloading', {
                progress: percent === null ? `${(downloaded / 1048576).toFixed(1)} MB` : `${percent}%`,
              })}</p>
              <div role="progressbar" aria-label={t('update.download')} aria-valuemin={0} aria-valuemax={100}
                aria-valuenow={percent ?? undefined} className="h-2 overflow-hidden rounded bg-secondary">
                <div className={`h-full bg-primary transition-all ${percent === null ? 'animate-pulse' : ''}`}
                  style={{ width: percent === null ? '35%' : `${percent}%` }} />
              </div>
            </div>
          )}
          {status === 'ready' && (
            <>
              <Button size="sm" disabled={!canInstall} onClick={() => void installUpdate()}>
                {t('update.installRestart')}
              </Button>
              {!canInstall && <p className="text-center text-xs text-muted-foreground">{t('update.waitForDownloads')}</p>}
            </>
          )}
          {status === 'installing' && <p className="flex items-center gap-2 text-sm">
            <Loader2 className="h-4 w-4 animate-spin" />{t('update.installing')}
          </p>}
          {!supported && <p className="text-center text-xs text-muted-foreground">
            {t(updateInfo.unsupportedReason === 'package-manager' ? 'update.packageManager' : 'update.manualInstall')}
          </p>}
          {!busy && <Button variant="link" size="sm" className="gap-2"
            onClick={() => void openUrl(updateInfo.releaseUrl)}>
            <ExternalLink className="h-3 w-3" />{t('update.releaseNotes')}
          </Button>}
        </>
      )}
      {error && <p role="alert" className="break-words text-center text-xs text-destructive">{error}</p>}
    </div>
  );
}
