import { telegram } from "../app/telegram";

const API_TIMEOUT_MS = 30_000;

export class ApiError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export const api = async <T>(path: string, init?: RequestInit): Promise<T> => {
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), API_TIMEOUT_MS);
  try {
    const response = await fetch(`/api${path}`, {
      headers: {
        "Content-Type": "application/json",
        // Inside Telegram, Telegram's signed user data is the login.
        ...(telegram ? { Authorization: `tma ${telegram.initData}` } : {}),
      },
      signal: controller.signal,
      ...init,
    });
    const body: unknown = await response.json().catch(() => undefined);
    if (!response.ok) {
      const message =
        (body as { error?: string } | undefined)?.error ||
        response.statusText ||
        `Request failed (${response.status})`;
      throw new ApiError(response.status, message);
    }
    return body as T;
  } catch (reason) {
    if (reason instanceof DOMException && reason.name === "AbortError") {
      throw new ApiError(0, "The request timed out. The backend may be busy.");
    }
    throw reason;
  } finally {
    window.clearTimeout(timer);
  }
};

/** POSTs `body` as JSON. */
export const post = <T = unknown>(path: string, body: unknown = {}) =>
  api<T>(path, { method: "POST", body: JSON.stringify(body) });
