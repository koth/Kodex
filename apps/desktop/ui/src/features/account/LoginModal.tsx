import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import {
  remoteControlLogin,
  remoteControlLogout,
  remoteControlSendLoginCode,
} from "../../lib/tauri";
import "./account.css";
import { PairingQrPanel } from "./PairingQrPanel";

interface Props {
  /** Login state captured when the modal opened. */
  loggedIn: boolean;
  accountEmail: string | null;
  onClose: () => void;
  /** Called after a successful login or logout so the caller can re-read
   * status and update the entry button. */
  onChanged: () => void;
}

type Step = "email" | "code";

/**
 * Passwordless email-OTP login modal.
 *
 * Two-step flow against the relay's `/auth/send-code` + `/auth/login` HTTP
 * endpoints (wired through the `remote_control_*` Tauri commands):
 *  1. email → "发送验证码" (POST /auth/send-code)
 *  2. code  → "登录"        (POST /auth/login → auth_token persisted by backend)
 * When already logged in, the modal shows the account email + "退出登录".
 * The relay's Chinese error strings (e.g. "验证码错误", "请求过于频繁")
 * surface verbatim so the user can act on them.
 */
export function LoginModal({ loggedIn, accountEmail, onClose, onChanged }: Props) {
  const [step, setStep] = useState<Step>("email");
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Bumps after a login/logout so the pairing panel re-mints its QR.
  const [pairingRefresh, setPairingRefresh] = useState(0);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const handleSendCode = useCallback(async () => {
    const trimmed = email.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await remoteControlSendLoginCode(trimmed);
      setStep("code");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [email, busy]);

  const handleLogin = useCallback(async () => {
    const trimmedEmail = email.trim();
    const trimmedCode = code.trim();
    if (!trimmedEmail || trimmedCode.length < 6 || busy) return;
    setBusy(true);
    setError(null);
    try {
      await remoteControlLogin(trimmedEmail, trimmedCode);
      setPairingRefresh((n) => n + 1);
      onChanged();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [email, code, busy, onChanged, onClose]);

  const handleLogout = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await remoteControlLogout();
      setPairingRefresh((n) => n + 1);
      onChanged();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [busy, onChanged, onClose]);

  return createPortal(
    <div
      className="account-modal-backdrop"
      role="presentation"
      onClick={onClose}
    >
      <div
        className="account-modal"
        role="dialog"
        aria-modal="true"
        aria-label={loggedIn ? "账号" : "登录远程控制"}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="account-modal-header">
          <span>{loggedIn ? "账号" : "登录远程控制"}</span>
          <button
            type="button"
            className="account-close"
            onClick={onClose}
            aria-label="关闭"
          >
            ×
          </button>
        </header>

        <div className="account-body">
          {loggedIn ? (
            <div className="account-logged-in">
              <span className="account-hint">已登录账号</span>
              <span className="account-email">{accountEmail ?? "—"}</span>
              {error && <div className="account-error">{error}</div>}
              <PairingQrPanel refreshKey={pairingRefresh} />
              <div className="account-actions">
                <button
                  type="button"
                  className="account-primary-btn"
                  onClick={handleLogout}
                  disabled={busy}
                >
                  {busy ? "退出中…" : "退出登录"}
                </button>
              </div>
            </div>
          ) : step === "email" ? (
            <>
              <div className="account-field">
                <label className="account-label" htmlFor="account-email-input">
                  邮箱
                </label>
                <input
                  id="account-email-input"
                  className="account-input"
                  type="email"
                  autoComplete="email"
                  placeholder="you@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void handleSendCode();
                  }}
                  autoFocus
                />
              </div>
              <p className="account-hint">
                登录后可绑定设备，使 PC 重启后免扫码重连远程控制。
              </p>
              {error && <div className="account-error">{error}</div>}
              <div className="account-actions">
                <button
                  type="button"
                  className="account-primary-btn"
                  onClick={handleSendCode}
                  disabled={!email.trim() || busy}
                >
                  {busy ? "发送中…" : "发送验证码"}
                </button>
              </div>
            </>
          ) : (
            <>
              <div className="account-field">
                <label className="account-label" htmlFor="account-code-input">
                  验证码
                </label>
                <input
                  id="account-code-input"
                  className="account-input account-code-input"
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  placeholder="······"
                  value={code}
                  onChange={(e) =>
                    setCode(e.target.value.replace(/[^0-9]/g, "").slice(0, 6))
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void handleLogin();
                  }}
                  autoFocus
                />
              </div>
              <p className="account-hint">
                已向 <span className="account-email">{email.trim()}</span>{" "}
                发送 6 位验证码，10 分钟内有效。
                <button
                  type="button"
                  className="account-link"
                  onClick={() => {
                    setStep("email");
                    setCode("");
                    setError(null);
                  }}
                >
                  换邮箱
                </button>
              </p>
              {error && <div className="account-error">{error}</div>}
              <div className="account-actions">
                <button
                  type="button"
                  className="account-secondary-btn"
                  onClick={() => setStep("email")}
                  disabled={busy}
                >
                  返回
                </button>
                <button
                  type="button"
                  className="account-primary-btn"
                  onClick={handleLogin}
                  disabled={code.trim().length < 6 || busy}
                >
                  {busy ? "登录中…" : "登录"}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
