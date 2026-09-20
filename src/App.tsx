import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Scaffold shell. The real layout and rule table land in tasks #8 and #9;
 * this exists to prove the frontend/backend IPC path end to end.
 */
export function App() {
  const [version, setVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string>("app_version")
      .then((v) => {
        setVersion(v);
      })
      .catch((e: unknown) => {
        setError(String(e));
      });
  }, []);

  return (
    <main className="shell">
      <h1>Shelv</h1>
      <p className="muted">
        {error !== null
          ? `IPC error: ${error}`
          : version !== null
            ? `core reachable — v${version}`
            : "connecting…"}
      </p>
    </main>
  );
}
