import { useCallback, useEffect, useState } from "react";
import { GitCommitHorizontal, Sparkles } from "lucide-react";
import {
  gitCommit,
  gitCommitAndPush,
  gitGenerateCommitMessage,
  gitPush,
} from "../../lib/tauri";
import { onCommitProgress } from "../../lib/events";
import "./CommitDialog.css";

export function CommitDialog({
  stagedCount,
  unstagedCount = 0,
  aheadCount = 0,
  allowPush = true,
  onClose,
  onCommitted,
}: {
  stagedCount: number;
  unstagedCount?: number;
  aheadCount?: number;
  allowPush?: boolean;
  onClose: () => void;
  onCommitted: () => void | Promise<void>;
}) {
  const [message, setMessage] = useState("");
  const [committing, setCommitting] = useState(false);
  const [commitAndPushing, setCommitAndPushing] = useState(false);
  const [pushing, setPushing] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [progress, setProgress] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const busy = committing || generating || commitAndPushing || pushing;
  const hasStaged = stagedCount > 0;
  const pushOnly = !hasStaged && aheadCount > 0 && allowPush;
  const canCommit = message.trim().length > 0 && hasStaged && !busy;
  const canPushOnly = pushOnly && !busy;
  const canPrimary = pushOnly ? canPushOnly : canCommit;

  const subtitleParts = (() => {
    if (pushOnly) {
      return [
        aheadCount === 1 ? "1 个本地提交待推送" : `${aheadCount} 个本地提交待推送`,
        "无需填写提交信息",
      ];
    }

    const parts: string[] = [];
    if (hasStaged) {
      parts.push(`${stagedCount} 个已暂存`);
    } else {
      parts.push("无已暂存文件");
    }
    if (unstagedCount > 0) {
      parts.push(`${unstagedCount} 个未暂存（不会提交）`);
    }
    if (aheadCount > 0 && allowPush) {
      parts.push(aheadCount === 1 ? "另有 1 个提交待推送" : `另有 ${aheadCount} 个提交待推送`);
    }
    parts.push("约定式提交，单行不超过 72 字符");
    return parts;
  })();

  const handleGenerate = useCallback(async () => {
    if (busy || !hasStaged) return;
    setGenerating(true);
    setProgress("正在启动 AI 会话…");
    setError(null);
    setStatus(null);
    try {
      const draft = await gitGenerateCommitMessage();
      setMessage(draft);
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
      setProgress(null);
    }
  }, [busy, hasStaged]);

  const handleCommit = useCallback(async () => {
    const trimmed = message.trim();
    if (!trimmed || busy || !hasStaged) return;
    setCommitting(true);
    setError(null);
    setStatus(null);
    try {
      await gitCommit(trimmed);
      await onCommitted();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommitting(false);
    }
  }, [message, busy, hasStaged, onCommitted, onClose]);

  const handleCommitAndPush = useCallback(async () => {
    const trimmed = message.trim();
    if (!trimmed || busy || !allowPush || !hasStaged) return;
    setCommitAndPushing(true);
    setError(null);
    setStatus(null);
    try {
      const result = await gitCommitAndPush(trimmed);
      setStatus(result || "已提交并推送到远程");
      await onCommitted();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommitAndPushing(false);
    }
  }, [message, busy, allowPush, hasStaged, onCommitted, onClose]);

  const handlePushOnly = useCallback(async () => {
    if (busy || !allowPush || !pushOnly) return;
    setPushing(true);
    setError(null);
    setStatus(null);
    try {
      const result = await gitPush();
      setStatus(result || "已推送到远程");
      await onCommitted();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setPushing(false);
    }
  }, [busy, allowPush, pushOnly, onCommitted, onClose]);

  const handlePrimary = useCallback(() => {
    if (pushOnly) {
      void handlePushOnly();
      return;
    }
    if (allowPush) {
      void handleCommitAndPush();
      return;
    }
    void handleCommit();
  }, [pushOnly, allowPush, handlePushOnly, handleCommitAndPush, handleCommit]);

  useEffect(() => {
    if (!generating) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    onCommitProgress((message) => setProgress(message)).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [generating]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        onClose();
        return;
      }
      if (event.key === "Enter" && !event.shiftKey && pushOnly && canPushOnly) {
        event.preventDefault();
        void handlePushOnly();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose, pushOnly, canPushOnly, handlePushOnly]);

  const primaryLabel = (() => {
    if (pushOnly) return pushing ? "推送中..." : "推送";
    if (allowPush) return commitAndPushing ? "提交并推送中..." : "提交并推送";
    return committing ? "提交中..." : "提交";
  })();

  const hint = pushOnly ? "Enter 推送 · Esc 取消" : "Enter 提交并推送 · Esc 取消";
  const title = pushOnly ? "推送" : "提交";

  return (
    <div
      className="commit-dialog-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onClose();
      }}
    >
      <div className="commit-dialog" role="dialog" aria-modal="true" aria-label={title}>
        <div className="commit-dialog-header">
          <div className="commit-dialog-icon" aria-hidden="true">
            <GitCommitHorizontal size={16} strokeWidth={2.1} />
          </div>
          <div className="commit-dialog-copy">
            <div className="commit-dialog-title">{title}</div>
            <div className="commit-dialog-subtitle">{subtitleParts.join(" · ")}</div>
          </div>
        </div>

        {!pushOnly && (
          <label className="commit-dialog-field">
            <span className="commit-dialog-label">提交信息</span>
            <div className="commit-dialog-input-row">
              <input
                type="text"
                className="commit-message-input"
                placeholder="feat: 简要描述本次改动"
                value={message}
                autoFocus
                maxLength={72}
                onChange={(event) => setMessage(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    handlePrimary();
                  }
                }}
                disabled={busy || !hasStaged}
              />
              <button
                type="button"
                className="commit-ai-button"
                title="用当前模型生成提交信息"
                onClick={() => void handleGenerate()}
                disabled={busy || !hasStaged}
              >
                {generating ? (
                  <span className="commit-dialog-spinner" aria-hidden="true" />
                ) : (
                  <Sparkles size={14} strokeWidth={2.1} aria-hidden="true" />
                )}
                <span>{generating ? "生成中" : "AI"}</span>
              </button>
            </div>
          </label>
        )}

        {!hasStaged && !pushOnly && (
          <div className="commit-dialog-error" role="status">
            没有已暂存文件。请先在审查面板暂存后再提交。
          </div>
        )}

        {generating && progress && (
          <div className="commit-progress" role="status" aria-live="polite">
            <span className="commit-dialog-spinner" aria-hidden="true" />
            <span>{progress}</span>
          </div>
        )}
        {status && (
          <div className="commit-dialog-status" role="status" aria-live="polite">
            {status}
          </div>
        )}
        {error && <div className="commit-dialog-error">{error}</div>}

        <div className="commit-dialog-footer">
          <span className="commit-dialog-hint">{hint}</span>
          <div className="commit-dialog-actions">
            <button
              type="button"
              className="commit-dialog-cancel"
              onClick={onClose}
              disabled={busy}
            >
              取消
            </button>
            {!pushOnly && allowPush && (
              <button
                type="button"
                className="commit-dialog-secondary"
                onClick={() => void handleCommit()}
                disabled={!canCommit}
              >
                {committing ? "仅提交中..." : "仅提交"}
              </button>
            )}
            <button
              type="button"
              className="commit-button"
              onClick={handlePrimary}
              disabled={!canPrimary}
            >
              {primaryLabel}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
