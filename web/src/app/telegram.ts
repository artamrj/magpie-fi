/** The Telegram Mini App bridge (telegram.org/js/telegram-web-app.js), when the page runs inside Telegram. */
type TelegramWebApp = {
  initData: string;
  initDataUnsafe: { start_param?: string };
  colorScheme: "light" | "dark";
  ready(): void;
  expand(): void;
  onEvent(event: "themeChanged", handler: () => void): void;
  showConfirm(message: string, callback: (confirmed: boolean) => void): void;
};

declare global {
  interface Window {
    Telegram?: { WebApp?: TelegramWebApp };
  }
}

/** Set only inside Telegram (outside it, the script loads but has no data). */
export const telegram: TelegramWebApp | undefined = window.Telegram?.WebApp?.initData
  ? window.Telegram.WebApp
  : undefined;

/** Asks yes/no: Telegram's own dialog inside the Mini App, the browser's elsewhere. */
export function askConfirm(message: string): Promise<boolean> {
  if (telegram) {
    return new Promise((resolve) => telegram!.showConfirm(message, resolve));
  }
  return Promise.resolve(window.confirm(message));
}

/** The view to open first: `?view=review` or `?view=get` (or the Mini App start parameter). */
export function startView(): string | null {
  return (
    new URLSearchParams(window.location.search).get("view") ??
    telegram?.initDataUnsafe.start_param ??
    null
  );
}
