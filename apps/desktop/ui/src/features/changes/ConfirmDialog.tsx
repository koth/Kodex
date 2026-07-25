import { useCallback, useEffect, useId, useRef } from "react";
import type { MouseEvent, ReactNode } from "react";
import "./ConfirmDialog.css";

export type ConfirmDialogTone = "default" | "danger";

export function ConfirmDialog({
  open,
  title,
  description,
  detail,
  confirmLabel = "确认",
  cancelLabel = "取消",
  tone = "default",
  busy = false,
  onConfirm,
  onCancel,
  icon,
}: {
  open: boolean;
  title: string;
  description: ReactNode;
  detail?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: ConfirmDialogTone;
  busy?: boolean;
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
  icon?: ReactNode;
}) {
  const titleId = useId();
  const descriptionId = useId();
  const confirmRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      confirmRef.current?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open]);

  useEffect(() => {
    if (!open) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (busy) return;
        event.preventDefault();
        onCancel();
        return;
      }

      if (event.key === "Enter") {
        if (busy) return;
        const target = event.target;
        if (
          target instanceof HTMLElement &&
          target.closest("textarea, [contenteditable='true']")
        ) {
          return;
        }
        event.preventDefault();
        void onConfirm();
      }
    };

    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [busy, onCancel, onConfirm, open]);

  const handleBackdropMouseDown = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget || busy) return;
      onCancel();
    },
    [busy, onCancel],
  );

  if (!open) return null;

  return (
    <div
      className="confirm-dialog-backdrop"
      role="presentation"
      onMouseDown={handleBackdropMouseDown}
    >
      <div
        className={`confirm-dialog ${tone === "danger" ? "is-danger" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <div className="confirm-dialog-header">
          {icon ? (
            <div className="confirm-dialog-icon" aria-hidden="true">
              {icon}
            </div>
          ) : null}
          <div className="confirm-dialog-copy">
            <div className="confirm-dialog-title" id={titleId}>
              {title}
            </div>
            <div className="confirm-dialog-description" id={descriptionId}>
              {description}
            </div>
            {detail ? <div className="confirm-dialog-detail">{detail}</div> : null}
          </div>
        </div>

        <div className="confirm-dialog-actions">
          <button
            type="button"
            className="confirm-dialog-button confirm-dialog-button-secondary"
            onClick={onCancel}
            disabled={busy}
          >
            {cancelLabel}
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={`confirm-dialog-button confirm-dialog-button-primary ${
              tone === "danger" ? "is-danger" : ""
            }`}
            onClick={() => void onConfirm()}
            disabled={busy}
          >
            {busy ? "处理中..." : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
