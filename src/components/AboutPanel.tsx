import { useEffect, useState } from "react";

import { appVersion, dataLocations, revealDataFolder, ShelvError } from "../lib/ipc";
import type { DataLocations } from "../types";

/**
 * What this build is, and where it keeps what it remembers.
 *
 * The paths exist here because there was nowhere else to find them. They
 * were only ever in a startup log line, which is no use to someone holding a
 * downloaded binary and wondering why it already knows their rules.
 */
export function AboutPanel({ onClose }: { onClose: () => void }) {
  const [version, setVersion] = useState<string | null>(null);
  const [locations, setLocations] = useState<DataLocations | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const load = async () => {
      try {
        const [nextVersion, nextLocations] = await Promise.all([
          appVersion(),
          dataLocations(),
        ]);
        setVersion(nextVersion);
        setLocations(nextLocations);
      } catch (e: unknown) {
        setError(e instanceof ShelvError ? e.message : String(e));
      }
    };
    void load();
  }, []);

  const reveal = async () => {
    setError(null);
    try {
      await revealDataFolder();
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-auto bg-black/60 p-8"
      role="dialog"
      aria-modal="true"
      aria-label="About Shelv"
    >
      <div className="w-full max-w-lg rounded border border-border bg-surface shadow-xl">
        <header className="flex items-center justify-between border-b border-border px-4 py-3">
          <h2 className="text-sm font-semibold">
            Shelv{version !== null && ` ${version}`}
          </h2>
          <button type="button" onClick={onClose} className="text-fg-muted hover:text-fg">
            Close
          </button>
        </header>

        <div className="space-y-4 p-4 text-[13px]">
          {error !== null && (
            <p className="text-xs" style={{ color: "var(--status-mismatch)" }}>
              {error}
            </p>
          )}

          <section className="space-y-2">
            <h3 className="text-xs font-medium tracking-wide text-fg-muted uppercase">
              Where your data is
            </h3>

            {locations === null ? (
              <p className="text-fg-muted">Loading…</p>
            ) : (
              <dl className="space-y-2">
                <Location label="Rules, drives and history" path={locations.database} />
                <Location label="Window preferences" path={locations.webview_profile} />
              </dl>
            )}

            {/* The question this panel was built to answer. Someone who
                replaces the binary and finds their rules still there is
                entitled to know that is deliberate rather than a ghost. */}
            <p className="text-xs text-fg-muted">
              These belong to your user account, not to the program file, so replacing
              Shelv with a newer build keeps every rule, drive and nickname. Nothing here
              is on your backup drives.
            </p>
          </section>

          <div className="flex justify-end border-t border-border pt-4">
            <button
              type="button"
              onClick={() => void reveal()}
              className="rounded px-3 py-1"
              style={{
                color: "var(--accent-blue)",
                backgroundColor: "var(--accent-blue-fill)",
              }}
            >
              Open data folder
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/** One labelled path, monospaced and selectable. */
function Location({ label, path }: { label: string; path: string }) {
  return (
    <div>
      <dt className="text-xs text-fg-muted">{label}</dt>
      {/* `select-all` so a click grabs the whole path: these are long, and
          the reason to look is almost always to paste it somewhere. */}
      <dd className="mt-0.5 rounded border border-border bg-bg px-2 py-1 font-mono text-[11px] break-all select-all">
        {path}
      </dd>
    </div>
  );
}
