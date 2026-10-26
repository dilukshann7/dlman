import { useState, useEffect } from 'react';
import { Github, Heart, Coffee } from 'lucide-react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';

const GITHUB_URL = 'https://github.com/dilukshann7/dlman';
const SPONSORS_URL = 'https://github.com/dilukshann7/dlman';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { useUIStore } from '@/stores/ui';
import { useUpdateStore } from '@/stores/update';
import { getAppVersion } from '@/lib/version';
import { AppUpdateControls } from '@/components/AppUpdateControls';

export function AboutDialog() {
  const { t } = useTranslation();
  const { showAboutDialog, setShowAboutDialog } = useUIStore();
  const markAboutSeen = useUpdateStore((s) => s.markAboutSeen);
  const [version, setVersion] = useState<string>('...');

  // Mark about as seen when dialog opens (dismisses update badge for this session)
  useEffect(() => {
    if (showAboutDialog) {
      markAboutSeen();
      getAppVersion().then((info) => setVersion(info.current));
    }
  }, [showAboutDialog, markAboutSeen]);


  return (
    <Dialog open={showAboutDialog} onOpenChange={setShowAboutDialog}>
      <DialogContent className="max-w-md">
        <DialogHeader className="text-center">
          <div className="flex justify-center mb-4">
            <div className="rounded-full bg-primary/10 p-4">
              <img src="/logo.png" alt="DLMan" className="h-12 w-12 object-contain" />
            </div>
          </div>
          <DialogTitle className="text-2xl text-center">DLMan</DialogTitle>
          <DialogDescription className="text-center">
            {t('about.subtitle')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="text-center">
            <p className="text-muted-foreground text-sm">{t('about.version', { version })}</p>
          </div>

          {/* Update Check Section */}
          <AppUpdateControls />

          <div className="text-center text-sm text-muted-foreground">
            <p>{t('about.tagline1')}</p>
            <p>{t('about.tagline2')}</p>
          </div>

          <div className="flex flex-col gap-2 items-center pt-4">
            <Button
              variant="outline"
              size="sm"
              className="gap-2"
              onClick={() => openUrl(GITHUB_URL)}
            >
              <Github className="h-4 w-4" />
              {t('about.viewGithub')}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="gap-2"
              onClick={() => openUrl(SPONSORS_URL)}
            >
              <Coffee className="h-4 w-4" />
              {t('about.buyCoffee')}
            </Button>
          </div>

          <div className="text-center text-xs text-muted-foreground pt-4">
            <p className="flex items-center justify-center gap-1">
              {t('about.madeWithPre')} <Heart className="h-3 w-3 text-red-500" /> {t('about.madeWithPost')}
            </p>
            <p className="mt-1">{t('about.copyright')}</p>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
