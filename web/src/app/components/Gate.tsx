import { useQuery } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
import type { Me } from "@/api/types";
import { Icon } from "../Icons";
import { telegram } from "../telegram";

/** Shows the page only to someone who may use it; otherwise explains how to log in. */
export function Gate({ children }: { children: React.ReactNode }) {
  const me = useQuery({
    queryKey: ["me"],
    queryFn: () => api<Me>("/me"),
    retry: false,
    staleTime: Infinity,
  });
  if (me.isPending) {
    return (
      <div className="gate">
        <span className="spinner" />
      </div>
    );
  }
  const status = me.error instanceof ApiError ? me.error.status : undefined;
  if (me.isError || me.data?.role !== "owner") {
    const guest = me.data?.role === "guest" || status === 403;
    return (
      <div className="gate">
        <div className="empty-state">
          <span>
            <Icon name={guest ? "music" : "shield"} size={28} />
          </span>
          <h2>{guest ? "This page is for owners" : "Log in to magpie"}</h2>
          <p>
            {guest
              ? "Send songs and links to the magpie bot in Telegram instead."
              : telegram
                ? "Telegram didn't confirm who you are. Close this page and open it again from the bot."
                : status === 401
                  ? "Send /web to your magpie bot in Telegram. It answers with a login link for this browser."
                  : "magpie isn't answering. Check that it is running."}
          </p>
          {!guest && status !== 401 && (
            <button className="compact-button" onClick={() => void me.refetch()}>
              <Icon name="refresh" size={15} />
              Try again
            </button>
          )}
        </div>
      </div>
    );
  }
  return <>{children}</>;
}
