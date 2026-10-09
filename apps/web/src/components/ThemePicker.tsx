import { THEMES } from '../lib/themes.js';
import { useUiStore } from '../store/uiStore.js';

/** One segment per registered theme; the selected one is pressed. */
export function ThemePicker() {
  const { theme, setTheme } = useUiStore();
  return (
    <div role="group" aria-label="Theme" className="flex rounded-md border border-subtle overflow-hidden w-fit text-sm">
      {THEMES.map(({ id, label, icon: Icon }) => (
        <button
          key={id}
          type="button"
          aria-pressed={theme === id}
          onClick={() => setTheme(id)}
          className={`inline-flex items-center gap-1.5 px-3 py-1.5 ${theme === id ? 'bg-accent-fill text-on-fill' : 'text-secondary hover:bg-overlay/5'}`}
        >
          <Icon size={14} aria-hidden="true" />
          {label}
        </button>
      ))}
    </div>
  );
}
