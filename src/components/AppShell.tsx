import type { ReactNode } from "react";

/**
 * The window frame: a header, a scrolling content area and a status bar.
 *
 * Only the content area scrolls, so the column headers of a long rule table
 * stay put and the status bar stays visible while a backup runs.
 */
export function AppShell({
  title,
  actions,
  status,
  statusRight,
  children,
}: {
  title: string;
  actions?: ReactNode;
  status?: ReactNode;
  /** Pinned to the right of the status bar, away from the running commentary. */
  statusRight?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex h-full flex-col bg-bg text-fg">
      <header
        className="flex shrink-0 items-center justify-between border-b border-border px-4"
        style={{ height: "var(--header-height)" }}
      >
        <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
        <div className="flex items-center gap-2">{actions}</div>
      </header>

      <main className="min-h-0 flex-1 overflow-auto">{children}</main>

      <footer
        className="flex shrink-0 items-center justify-between gap-4 border-t border-border px-4 text-xs text-fg-muted"
        style={{ height: "var(--statusbar-height)" }}
      >
        <span className="truncate">{status}</span>
        {statusRight}
      </footer>
    </div>
  );
}
