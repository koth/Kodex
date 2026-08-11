import { useCallback, useEffect, useRef, useState } from "react";
import QRCode from "qrcode";
import {
  remoteControlPairingQr,
  remoteControlStatus,
} from "../../lib/tauri";

interface Props {
  refreshKey: number;
}

type Status = "idle" | "minting" | "ready" | "error";

export function PairingQrPanel({ refreshKey }: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);
  const [refreshIn, setRefreshIn] = useState(0);

  const mint = useCallback(async () => {
    setStatus("minting");
    setError(null);
    try {
      const json = await remoteControlPairingQr();
      if (!json) {
        setStatus("error");
        setError("远程控制已禁用");
        return;
      }
      if (canvasRef.current) {
        await QRCode.toCanvas(canvasRef.current, json, {
          width: 220,
          margin: 2,
          errorCorrectionLevel: "M",
          color: { dark: "#0b0c0e", light: "#ffffff" },
        });
      }
      setStatus("ready");
      setRefreshIn(100);
    } catch (e) {
      setStatus("error");
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void mint();
  }, [mint, refreshKey]);

  useEffect(() => {
    let active = true;
    const poll = async () => {
      try {
        const s = await remoteControlStatus();
        if (active) setConnected(s.connected);
      } catch {
        // best-effort; keep last known state
      }
    };
    void poll();
    const id = window.setInterval(poll, 3000);
    return () => {
      active = false;
      window.clearInterval(id);
    };
  }, []);

  useEffect(() => {
    if (status !== "ready") return;
    const id = window.setInterval(() => {
      setRefreshIn((n) => {
        if (n <= 1) {
          void mint();
          return 100;
        }
        return n - 1;
      });
    }, 1000);
    return () => window.clearInterval(id);
  }, [status, mint]);

  return (
    <div className="account-pairing">
      <span className="account-hint">配对二维码</span>
      <div className="account-pairing-body">
        <canvas
          ref={canvasRef}
          className="account-qr-canvas"
          aria-label="PC 配对二维码"
          role="img"
        />
        <div className="account-pairing-meta">
          <p className="account-pairing-step">
            1. 在手机 Maju app 打开「配对」，扫描上方二维码。
          </p>
          <p className="account-pairing-step">
            2. 配对成功后 PC 会自动保持连接，手机可远程控制。
          </p>
          <p
            className={`account-pairing-conn ${connected ? "is-on" : "is-off"}`}
          >
            {connected ? "● 已连接 relay" : "○ 未连接 relay"}
          </p>
          {status === "ready" && (
            <p className="account-pairing-ttl">二维码 {refreshIn}s 后刷新</p>
          )}
          {status === "minting" && (
            <p className="account-pairing-ttl">生成中…</p>
          )}
          {status === "error" && <div className="account-error">{error}</div>}
          <button
            type="button"
            className="account-secondary-btn"
            onClick={() => void mint()}
            disabled={status === "minting"}
          >
            手动刷新
          </button>
        </div>
      </div>
    </div>
  );
}
