import { usePopupStore } from '../store';
import { Power } from 'lucide-react';

export function Header() {
  const { settings, toggleEnabled } = usePopupStore();

  return (
    <header className="flex items-center justify-between px-4 py-3 border-b bg-card">
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 flex items-center justify-center">
          <img src="/icon/128.png" alt="DLMan" className="w-8 h-8 object-contain" />
        </div>
        <div>
          <h1 className="font-semibold text-base">DLMan</h1>
          <p className="text-xs text-muted-foreground">Download Manager</p>
        </div>
      </div>
      
      <button
        onClick={toggleEnabled}
        className={`
          p-2 rounded-lg transition-colors
          ${settings?.enabled 
            ? 'bg-primary/10 text-primary hover:bg-primary/20' 
            : 'bg-muted text-muted-foreground hover:bg-muted/80'
          }
        `}
        title={settings?.enabled ? 'Disable extension' : 'Enable extension'}
      >
        <Power className="w-5 h-5" />
      </button>
    </header>
  );
}
