/** Keep the window visible until the worker has produced a cleanup report. */

import { cleanupWarning, shutdownError } from './operations.js';

export async function installCloseGuard(windowHandle, handlers) {
  if (!windowHandle?.onCloseRequested || !windowHandle?.close) {
    throw new Error('Tauri window close events are unavailable');
  }
  let closing = false;
  let unlisten = async () => {};
  unlisten = await windowHandle.onCloseRequested(async (event) => {
    if (!handlers.active()) return;
    // Tauri requires this before awaiting anything; otherwise the window may
    // disappear while a serial/VISA cleanup is still running.
    event.preventDefault();
    if (closing) return;
    closing = true;
    try {
      if (!await handlers.confirm()) return;
      const result = await handlers.stop();
      const warning = cleanupWarning(result);
      await handlers.report(result, warning);
      if (!warning) {
        await unlisten();
        await windowHandle.close();
      }
    } catch (error) {
      await handlers.error(shutdownError(error));
    } finally {
      closing = false;
    }
  });
  return unlisten;
}
