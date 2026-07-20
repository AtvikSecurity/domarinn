import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, RouterProvider } from "react-router";
import { ApiError } from "@/api/client";
import { AuthProvider } from "@/auth/AuthProvider";
import { RequireAdmin } from "@/auth/guards";
import { Layout } from "@/components/Layout";
import { TokenModal } from "@/components/TokenModal";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { NotFound } from "@/components/NotFound";
import { RunsList } from "@/pages/RunsList";
import { RunDetail } from "@/pages/RunDetail";
import { ComparePage } from "@/pages/ComparePage";
import { CacheStatsPage } from "@/pages/CacheStatsPage";
import { SettingsPage } from "@/pages/SettingsPage";
import { LoginPage } from "@/pages/LoginPage";
import { SetupPage } from "@/pages/SetupPage";
import { KeysPage } from "@/pages/KeysPage";
import { AdminPage } from "@/pages/AdminPage";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
      retry: (failureCount, error) => {
        // Never retry auth failures; the token modal handles those.
        if (error instanceof ApiError && (error.status === 401 || error.status === 404)) {
          return false;
        }
        return failureCount < 2;
      },
    },
  },
});

const router = createBrowserRouter([
  {
    path: "/",
    element: <Layout />,
    children: [
      { index: true, element: <RunsList /> },
      { path: "runs/:id", element: <RunDetail /> },
      // No target-less `runs/:id/compare` route: the real server route is
      // `Path((id, other))` and requires both segments, so there is nothing
      // useful to render without a resolved comparison target. Compare links
      // resolve a target from already-loaded data before navigating (see
      // RunsList/RunDetail) and 404 via the catch-all below otherwise.
      { path: "runs/:id/compare/:other", element: <ComparePage /> },
      { path: "cache", element: <CacheStatsPage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "login", element: <LoginPage /> },
      { path: "setup", element: <SetupPage /> },
      { path: "keys", element: <KeysPage /> },
      {
        path: "admin",
        element: (
          <RequireAdmin>
            <AdminPage />
          </RequireAdmin>
        ),
      },
      { path: "*", element: <NotFound /> },
    ],
  },
]);

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <AuthProvider>
          <RouterProvider router={router} />
          <TokenModal />
        </AuthProvider>
      </TooltipProvider>
    </QueryClientProvider>
  );
}
