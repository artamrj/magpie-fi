import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { Gate } from "./components/Gate";
import { telegram } from "./telegram";
import "../styles/index.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

// Inside Telegram: show the page right away, at full height.
telegram?.ready();
telegram?.expand();

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}>
    <Gate>
      <App />
    </Gate>
  </QueryClientProvider>,
);
