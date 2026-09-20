import { useEffect, useState } from "react";

import { appVersion, listRules, listVolumes } from "./lib/ipc";

/**
 * Scaffold shell. The real layout and rule table land in tasks #8 and #9;
 * this exists to prove the frontend/backend IPC path end to end.
 */
export function App() {
  const [status, setStatus] = useState<string>("connecting…");

  useEffect(() => {
    // Exercises a plain command, a store read and a platform read, so the
    // whole path is proven before the real UI lands in tasks #8 and #9.
    void (async () => {
      try {
        const [version, rules, volumes] = await Promise.all([
          appVersion(),
          listRules(),
          listVolumes(),
        ]);
        setStatus(
          `v${version} — ${String(rules.length)} rules, ${String(volumes.length)} known volumes`,
        );
      } catch (error: unknown) {
        setStatus(`IPC error: ${String(error)}`);
      }
    })();
  }, []);

  return (
    <main className="shell">
      <h1>Shelv</h1>
      <p className="muted">{status}</p>
    </main>
  );
}
