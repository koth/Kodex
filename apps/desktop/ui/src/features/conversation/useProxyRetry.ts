import { useEffect, useState } from "react";
import type { ProxyRetryStatus } from "../../types";
import { onProxyRetry } from "../../lib/events";

/**
 * Subscribes to the `proxy:retry` event pushed by the Tauri snapshot bridge
 * and returns the current upstream-retry status for the active session, or
 * `null` when no retry is in flight. The codex_api_proxy publishes each retry
 * attempt (502 / 429 / transport error) so the UI can render an animation.
 */
export function useProxyRetry(): ProxyRetryStatus | null {
  const [status, setStatus] = useState<ProxyRetryStatus | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    onProxyRetry((next) => {
      if (disposed) return;
      setStatus(next);
    })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
          return;
        }
        unlisten = cleanup;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return status;
}

/** Human-readable Chinese label for a retry reason. */
export function proxyRetryReasonLabel(reason: string): string {
  switch (reason) {
    case "rate_limited":
      return "上游限流";
    case "bad_gateway":
      return "上游网关错误";
    case "service_unavailable":
      return "上游暂时不可用";
    case "gateway_timeout":
      return "上游网关超时";
    case "internal_server_error":
    case "server_error":
      return "上游服务异常";
    case "cloudflare_web_unknown":
    case "cloudflare_web_down":
    case "cloudflare_origin_unreachable":
      return "上游源站不可达";
    case "cloudflare_connection_timeout":
    case "cloudflare_timeout":
      return "上游响应超时";
    case "transport_error":
      return "网络异常";
    default:
      return "上游异常";
  }
}
