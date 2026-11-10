import { ApiError } from "../../api/client";
import { useMeQuery } from "../../api/queries";
import { telegram } from "../telegram";
import { EmptyState } from "./EmptyStates";

/** Shows the page only to someone who may use it; otherwise explains how to log in. */
export function Gate({ children }: { children: React.ReactNode }) {
  const me = useMeQuery();
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
        <EmptyState
          icon={guest ? "music" : "shield"}
          title={guest ? "This page is for owners" : "Log in to magpie-fi"}
          description={
            guest
              ? "Send songs and links to the magpie-fi bot in Telegram instead."
              : telegram
                ? "Telegram didn't confirm who you are. Close this page and open it again from the bot."
                : status === 401
                  ? "Send /web to your magpie-fi bot in Telegram. It answers with a login link for this browser."
                  : "magpie-fi isn't answering. Check that it is running."
          }
          action={!guest && status !== 401 ? "Try again" : undefined}
          onAction={() => void me.refetch()}
        />
      </div>
    );
  }
  return <>{children}</>;
}
