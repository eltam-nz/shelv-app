import { useEffect, useState } from "react";

import { AppShell } from "./components/AppShell";
import { AvailabilityPill, RunResultPill } from "./components/StatusPill";
import { appVersion, listRules, listVolumes } from "./lib/ipc";
import { TAG_TOKENS, tagStyle } from "./lib/palette";
import type { Availability, RunResult } from "./types";

const AVAILABILITIES: Availability[] = [
  "available",
  "disconnected",
  "refused",
  "identity_mismatch",
];
const RESULTS: (RunResult | null)[] = [null, "ok", "partial", "failed", "cancelled"];

/**
 * Theme preview.
 *
 * Stands in until the rule table lands in task #9, and renders every palette
 * token and status state so a regression is visible rather than theoretical.
 */
export function App() {
  const [status, setStatus] = useState<string>("connecting…");

  useEffect(() => {
    void (async () => {
      try {
        const [version, rules, volumes] = await Promise.all([
          appVersion(),
          listRules(),
          listVolumes(),
        ]);
        setStatus(
          `v${version} · ${String(rules.length)} rules · ${String(volumes.length)} known volumes`,
        );
      } catch (error: unknown) {
        setStatus(`IPC error: ${String(error)}`);
      }
    })();
  }, []);

  return (
    <AppShell title="Shelv" status={status}>
      <div className="space-y-6 p-4">
        <Section title="Tags">
          <div className="flex flex-wrap gap-2">
            {TAG_TOKENS.map((token) => (
              <span
                key={token}
                className="rounded px-2 py-0.5 text-xs"
                style={tagStyle(token)}
              >
                {token.replace("pastel-", "")}
              </span>
            ))}
          </div>
        </Section>

        <Section title="Destination status">
          <div className="flex flex-wrap gap-3">
            {AVAILABILITIES.map((a) => (
              <AvailabilityPill key={a} availability={a} />
            ))}
          </div>
        </Section>

        <Section title="Last result">
          <div className="flex flex-wrap gap-3">
            {RESULTS.map((r) => (
              <RunResultPill key={r ?? "never"} result={r} />
            ))}
          </div>
        </Section>

        <Section title="Surfaces">
          <div className="flex gap-3">
            {["bg", "surface", "surface-raised"].map((name) => (
              <div
                key={name}
                className="rounded border border-border px-3 py-6 text-xs text-fg-muted"
                style={{ backgroundColor: `var(--${name})` }}
              >
                {name}
              </div>
            ))}
          </div>
        </Section>
      </div>
    </AppShell>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded border border-border bg-surface p-4">
      <h2 className="mb-3 text-xs font-medium tracking-wide text-fg-muted uppercase">
        {title}
      </h2>
      {children}
    </section>
  );
}
