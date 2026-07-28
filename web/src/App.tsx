import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, RouterProvider } from "react-router";
import { ApiError } from "@/api/client";
import { AuthProvider } from "@/auth/AuthProvider";
import { RequireAdmin, RequireAuth } from "@/auth/guards";
import { Layout } from "@/components/Layout";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { NotFound } from "@/components/NotFound";
import { RouteError } from "@/components/RouteError";
import { OverviewPage } from "@/pages/OverviewPage";
import { RunsList } from "@/pages/RunsList";
import { RunDetail } from "@/pages/RunDetail";
import { ComparePage } from "@/pages/ComparePage";
import { SearchPage } from "@/pages/SearchPage";
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
    // Last-resort boundary: also catches errors thrown by Layout itself, so
    // it renders without the app chrome.
    errorElement: <RouteError />,
    children: [
      // Public: the only pages an unauthenticated visitor may reach in
      // closed mode. Both keep their own setup/session redirects.
      { path: "login", element: <LoginPage /> },
      { path: "setup", element: <SetupPage /> },
      // Everything else is behind RequireAuth (a pathless layout route). The
      // catch-all lives inside it too, so an unknown path redirects an
      // anonymous visitor to /login rather than leaking that the route does
      // not exist.
      {
        element: <RequireAuth />,
        // Page-level crashes render inside Layout's Outlet, keeping the
        // header and navigation usable.
        errorElement: <RouteError />,
        children: [
          // `/` is the status surface and `/runs` is the stream. Two routes
          // rather than one page, because they sort incompatibly: the stream is
          // newest-first by definition, while a status surface must stay put
          // when somebody iterates locally.
          { index: true, element: <OverviewPage /> },
          { path: "runs", element: <RunsList /> },
          { path: "runs/:id", element: <RunDetail /> },
          // No target-less `runs/:id/compare` route: the real server route is
          // `Path((id, other))` and requires both segments, so there is
          // nothing useful to render without a resolved comparison target.
          // Compare links resolve a target from already-loaded data before
          // navigating (see RunsList/RunDetail) and 404 via the catch-all
          // below otherwise.
          { path: "runs/:id/compare/:other", element: <ComparePage /> },
          { path: "search", element: <SearchPage /> },
          { path: "cache", element: <CacheStatsPage /> },
          { path: "settings", element: <SettingsPage /> },
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
    ],
  },
]);

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <AuthProvider>
          <RouterProvider router={router} />
        </AuthProvider>
      </TooltipProvider>
    </QueryClientProvider>
  );
}
