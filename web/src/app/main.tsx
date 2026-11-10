import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { Gate } from "./components/Gate";
import { setupTelegram, telegram } from "./telegram";
import "../styles/index.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      // Coming back to the page (or to the Mini App) shows the newest state.
      refetchOnWindowFocus: true,
      refetchOnReconnect: true,
    },
  },
});

// Inside Telegram: full height, Telegram's colours, and its light/dark choice.
setupTelegram();
if (telegram) {
  document.documentElement.classList.add("in-telegram");
  const followTheme = () => {
    document.documentElement.dataset.theme = telegram!.colorScheme;
  };
  followTheme();
  telegram.onEvent("themeChanged", followTheme);
  // Telegram keeps a Mini App alive in the background; reload everything when it comes back.
  telegram.onEvent("activated", () => void queryClient.invalidateQueries());
} else {
  // Older versions saved a theme choice; the page now follows the system.
  localStorage.removeItem("magpie-fi-theme");
  localStorage.removeItem("magpie-theme");
}

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") void queryClient.invalidateQueries();
});

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}>
    <Gate>
      <App />
    </Gate>
  </QueryClientProvider>,
);
