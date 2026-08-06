import { useCallback, useEffect, useState } from "react";
import { remoteControlStatus, type RemoteControlStatus } from "../../lib/tauri";

/**
 * Surfaces the remote-control account state (`logged_in` + `account_email`)
 * from the backend `remote_control_status` command. Fetches once on mount;
 * callers call `refresh()` after a login/logout to re-read. A transport
 * failure (e.g. Tauri not ready in tests) degrades to logged-out, never
 * throws into the render path.
 */
export function useAccountStatus() {
  const [status, setStatus] = useState<RemoteControlStatus | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await remoteControlStatus());
    } catch {
      setStatus(null);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return {
    loggedIn: status?.logged_in ?? false,
    accountEmail: status?.account_email ?? null,
    refresh,
  };
}
