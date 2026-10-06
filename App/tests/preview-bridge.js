// Injected only by App.tests.preview_server. The production bundle never has
// this file or a network listener; the Python side rejects real mode.
document.title = 'Yang LAB INSTRUMENT CONSOLE · Simulated Browser Preview';
// Only this simulation fixture auto-accepts native confirms, so browser QA
// can exercise each path without a blocking OS dialog. Preserve the text for
// inspection; the bundled Tauri application retains its real confirmations.
globalThis.__previewConfirmations = [];
globalThis.confirm = (message) => {
  globalThis.__previewConfirmations.push(String(message));
  return true;
};
globalThis.__TAURI__ = {
  core: {
    async invoke(command, args = {}) {
      const response = await fetch('/__preview_invoke', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ command, args }),
      });
      const payload = await response.json();
      if (!response.ok) throw new Error(payload.error || 'preview request failed');
      return payload;
    },
  },
};
