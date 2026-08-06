import { useState } from "react";
import { useAccountStatus } from "./useAccountStatus";
import { LoginModal } from "./LoginModal";
import "./account.css";

/**
 * Footer entry for the remote-control account: shows live login state
 * (logged-out "登录" vs a green dot + "已登录") and opens the
 * {@link LoginModal} on click. Self-contained — fetches its own status on
 * mount and refreshes after a login/logout, so the parent (SessionList
 * footer) needs no new props.
 */
export function AccountButton() {
  const { loggedIn, accountEmail, refresh } = useAccountStatus();
  const [open, setOpen] = useState(false);

  return (
    <>
      <button
        type="button"
        className={`account-btn ${loggedIn ? "is-logged-in" : ""}`}
        onClick={() => setOpen(true)}
        title={loggedIn ? `已登录：${accountEmail ?? ""}` : "登录远程控制"}
        aria-label={loggedIn ? "账号" : "登录远程控制"}
      >
        <span className="account-btn-icon" aria-hidden="true">
          <AccountIcon />
        </span>
        <span className="account-btn-label">
          {loggedIn ? "已登录" : "登录"}
        </span>
        <span className="account-status-dot" aria-hidden="true" />
      </button>
      {open && (
        <LoginModal
          loggedIn={loggedIn}
          accountEmail={accountEmail}
          onClose={() => setOpen(false)}
          onChanged={refresh}
        />
      )}
    </>
  );
}

function AccountIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
      <path
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8ZM5 20a7 7 0 0 1 14 0"
      />
    </svg>
  );
}
