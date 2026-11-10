import { useEffect, useRef } from "react";

/** One of Telegram's bottom buttons (telegram.org/js/telegram-web-app.js, Bot API 7.10+). */
type BottomButton = {
  text: string;
  isVisible: boolean;
  isActive: boolean;
  setText(text: string): BottomButton;
  show(): BottomButton;
  hide(): BottomButton;
  enable(): BottomButton;
  disable(): BottomButton;
  showProgress(leaveActive?: boolean): BottomButton;
  hideProgress(): BottomButton;
  onClick(handler: () => void): BottomButton;
  offClick(handler: () => void): BottomButton;
  setParams(params: {
    text?: string;
    is_active?: boolean;
    is_visible?: boolean;
    color?: string;
    text_color?: string;
    position?: "left" | "right" | "top" | "bottom";
  }): BottomButton;
};

/** The Telegram Mini App bridge, when the page runs inside Telegram. */
type TelegramWebApp = {
  initData: string;
  colorScheme: "light" | "dark";
  platform: string;
  themeParams: Record<string, string>;
  ready(): void;
  expand(): void;
  setHeaderColor?(color: string): void;
  setBackgroundColor?(color: string): void;
  setBottomBarColor?(color: string): void;
  isVersionAtLeast(version: string): boolean;
  onEvent(event: "themeChanged" | "viewportChanged", handler: () => void): void;
  offEvent(event: "themeChanged" | "viewportChanged", handler: () => void): void;
  showConfirm(message: string, callback: (confirmed: boolean) => void): void;
  MainButton: BottomButton;
  SecondaryButton?: BottomButton;
  BackButton: {
    isVisible: boolean;
    show(): void;
    hide(): void;
    onClick(handler: () => void): void;
    offClick(handler: () => void): void;
  };
  HapticFeedback?: {
    impactOccurred(style: "light" | "medium" | "heavy" | "rigid" | "soft"): void;
    notificationOccurred(type: "error" | "success" | "warning"): void;
    selectionChanged(): void;
  };
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

/** Telegram's own bottom bar exists only in clients from Bot API 6.1 up; older ones get in-page buttons. */
export const hasTelegramButtons = Boolean(telegram?.MainButton && telegram.isVersionAtLeast("6.1"));
const hasSecondaryButton = Boolean(telegram?.SecondaryButton && telegram.isVersionAtLeast("7.10"));

/** Prepares the page inside Telegram: full height, Telegram's colours for the header and background. */
export function setupTelegram() {
  if (!telegram) return;
  telegram.ready();
  telegram.expand();
  try {
    telegram.setHeaderColor?.("secondary_bg_color");
    telegram.setBackgroundColor?.("secondary_bg_color");
    telegram.setBottomBarColor?.("secondary_bg_color");
  } catch {
    // Older clients reject colour names; the defaults are fine.
  }
}

/** Asks yes/no: Telegram's own dialog inside the Mini App, the browser's elsewhere. */
export function askConfirm(message: string): Promise<boolean> {
  if (telegram) {
    return new Promise((resolve) => telegram!.showConfirm(message, resolve));
  }
  return Promise.resolve(window.confirm(message));
}

export const haptic = {
  tap: () => telegram?.HapticFeedback?.selectionChanged(),
  success: () => telegram?.HapticFeedback?.notificationOccurred("success"),
  error: () => telegram?.HapticFeedback?.notificationOccurred("error"),
};

/** The view a bot link asks to open first (`?view=review`), and the song it points at (`&track=12`). */
const startParams = new URLSearchParams(window.location.search);
export const startView = startParams.get("view");
export const startTrack = Number(startParams.get("track")) || undefined;

/** What a bottom button does. `busy` shows a spinner; `disabled` greys it out. */
export type ButtonAction = {
  text: string;
  onClick: () => void;
  disabled?: boolean;
  busy?: boolean;
  destructive?: boolean;
};

function bindButton(button: BottomButton | undefined, action: ButtonAction | undefined) {
  if (!button) return;
  if (!action) {
    button.hideProgress();
    button.hide();
    return;
  }
  button.setParams({ text: action.text, is_visible: true, is_active: !action.disabled });
  if (action.busy) button.showProgress(false);
  else button.hideProgress();
}

/**
 * Drives Telegram's Main and Secondary buttons from React state. Outside Telegram this does
 * nothing: the screens render the same actions as in-page buttons (see `BottomActions`).
 */
export function useTelegramButtons(primary?: ButtonAction, secondary?: ButtonAction) {
  const primaryRef = useRef(primary);
  const secondaryRef = useRef(secondary);
  useEffect(() => {
    primaryRef.current = primary;
    secondaryRef.current = secondary;
  });

  useEffect(() => {
    if (!hasTelegramButtons || !telegram) return;
    const main = telegram.MainButton;
    const second = hasSecondaryButton ? telegram.SecondaryButton : undefined;
    const onMain = () => primaryRef.current?.onClick();
    const onSecond = () => secondaryRef.current?.onClick();
    main.onClick(onMain);
    second?.onClick(onSecond);
    second?.setParams({ position: "left" });
    return () => {
      main.offClick(onMain);
      second?.offClick(onSecond);
      main.hideProgress();
      main.hide();
      second?.hideProgress();
      second?.hide();
    };
  }, []);

  // Only what Telegram shows matters for rebinding, not the handlers' identity.
  const primaryKey = actionKey(primary);
  const secondaryKey = actionKey(secondary);
  useEffect(() => {
    if (!hasTelegramButtons || !telegram) return;
    bindButton(telegram.MainButton, primaryRef.current);
    bindButton(hasSecondaryButton ? telegram.SecondaryButton : undefined, secondaryRef.current);
  }, [primaryKey, secondaryKey]);
}

function actionKey(action?: ButtonAction) {
  return action ? `${action.text}|${action.disabled ? 1 : 0}|${action.busy ? 1 : 0}` : "";
}

/** Shows Telegram's back arrow while `onBack` is set; the page draws its own arrow elsewhere. */
export function useTelegramBackButton(onBack?: () => void) {
  const handlerRef = useRef(onBack);
  useEffect(() => {
    handlerRef.current = onBack;
  });
  useEffect(() => {
    if (!telegram) return;
    const back = telegram.BackButton;
    const handle = () => handlerRef.current?.();
    back.onClick(handle);
    return () => back.offClick(handle);
  }, []);
  const shown = Boolean(onBack);
  useEffect(() => {
    if (!telegram) return;
    if (shown) telegram.BackButton.show();
    else telegram.BackButton.hide();
  }, [shown]);
}
