import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { tileLabel } from "./tile";
import "./App.css";

function App() {
  const [hand, setHand] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string[]>("sample_hand")
      .then(setHand)
      .catch((err: unknown) => setError(String(err)));
  }, []);

  return (
    <main className="panel">
      <header className="titlebar" data-tauri-drag-region>
        <span className="status-dot" data-tauri-drag-region />
        <span data-tauri-drag-region>麻将 Copilot</span>
        <span className="muted" data-tauri-drag-region>
          示例手牌
        </span>
      </header>
      {error ? (
        <p className="error">{error}</p>
      ) : (
        <div className="hand">
          {hand.map((tile, i) => {
            const label = tileLabel(tile);
            return (
              <div key={i} className={`tile${label.red ? " red" : ""}`}>
                <span>{label.rank}</span>
                {label.suit && <small>{label.suit}</small>}
              </div>
            );
          })}
        </div>
      )}
    </main>
  );
}

export default App;
