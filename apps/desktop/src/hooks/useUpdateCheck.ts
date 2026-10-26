import { useEffect } from 'react';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import { useUpdateStore } from '@/stores/update';
import { useSettingsStore } from '@/stores/settings';
import { useUIStore } from '@/stores/ui';
import { startUpdateSchedule } from '@/lib/update-scheduler';

export function useUpdateCheck() {
  const { t } = useTranslation();
  const loaded = useSettingsStore(state => state.loaded);
  const enabled = useSettingsStore(state => state.settings.auto_check_updates);
  const check = useUpdateStore(state => state.checkForUpdates);
  const status = useUpdateStore(state => state.status);
  const refreshReadiness = useUpdateStore(state => state.refreshReadiness);
  useEffect(() => startUpdateSchedule({
    loaded, enabled,
    check: async () => {
      await check();
      const state = useUpdateStore.getState();
      if (state.status === 'available' && state.shouldShowNotification()) {
        toast.info(t('about.newVersion', { version: state.updateInfo?.latestVersion }), {
          id: 'app-update-available', description: t('update.clickToDownload'), duration: 10000,
          action: {
            label: t('common.update'),
            onClick: () => useUIStore.getState().setShowAboutDialog(true),
          },
        });
      }
    },
  }), [loaded, enabled, check, t]);
  useEffect(() => {
    if (status !== 'ready') return;
    void refreshReadiness();
    const timer = setInterval(() => void refreshReadiness(), 1000);
    return () => clearInterval(timer);
  }, [status, refreshReadiness]);
}
