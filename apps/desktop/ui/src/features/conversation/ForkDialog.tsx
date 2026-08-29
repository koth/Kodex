import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { GitFork } from "lucide-react";
import {
  sessionForkCandidates,
  type SessionForkCandidate,
} from "../../lib/tauri";
import type { ConversationForkMode } from "./ConversationTimeline";

interface ForkDialogProps {
  /** Turn-opening user message id preselected from the clicked fork button
   *  (null → nothing preselected; the user picks a turn from the list). */
  initialMessageId: string | null;
  /** Whether the "new worktree" destination is offered for this agent. */
  worktreeSupported: boolean;
  onFork: (messageId: string, mode: ConversationForkMode) => Promise<void> | void;
  onClose: () => void;
}

/**
 * Fork point picker ("从这里创建聊天分支"): lists EVERY completed turn of the
 * session — full persisted history, not just the UI's tail window — and lets
 * the user anchor the branch cut on any turn before choosing the hosting mode
 * (current workspace / new worktree).
 */
export function ForkDialog({
  initialMessageId,
  worktreeSupported,
  onFork,
  onClose,
}: ForkDialogProps) {
  const [candidates, setCandidates] = useState<SessionForkCandidate[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(initialMessageId);
  const [busyMode, setBusyMode] = useState<ConversationForkMode | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    sessionForkCandidates()
      .then((list) => {
        if (disposed) return;
        setCandidates(list);
        // A preselected turn may not exist in the (rebuilt) candidate list —
        // fall back to nothing selected instead of forking a stale anchor.
        setSelectedId((current) =>
          current && list.some((candidate) => candidate.user_message_id === current)
            ? current
            : null,
        );
      })
      .catch((err) => {
        if (!disposed) setLoadError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busyMode) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busyMode, onClose]);

  const run = useCallback(
    async (mode: ConversationForkMode) => {
      if (!selectedId || busyMode) return;
      setBusyMode(mode);
      setError(null);
      try {
        await onFork(selectedId, mode);
        onClose();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusyMode(null);
      }
    },
    [selectedId, busyMode, onFork, onClose],
  );

  return createPortal(
    <div
      className="fork-dialog-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busyMode) onClose();
      }}
    >
      <div className="fork-dialog" role="dialog" aria-modal="true" aria-label="从这里创建聊天分支">
        <div className="fork-dialog-header">
          <div className="fork-dialog-icon" aria-hidden="true">
            <GitFork size={16} strokeWidth={2.1} />
          </div>
          <div className="fork-dialog-copy">
            <div className="fork-dialog-title">从这里创建聊天分支</div>
            <div className="fork-dialog-subtitle">
              选择要保留的对话轮次，新分支将携带该轮及之前的全部上下文
            </div>
          </div>
        </div>

        <div className="fork-dialog-list" role="listbox" aria-label="可分叉的对话轮次">
          {candidates === null && !loadError && (
            <div className="fork-dialog-empty">正在加载会话记录…</div>
          )}
          {loadError && <div className="fork-dialog-empty">加载会话记录失败：{loadError}</div>}
          {candidates !== null && candidates.length === 0 && (
            <div className="fork-dialog-empty">该会话还没有可分叉的对话轮次</div>
          )}
          {candidates?.map((candidate) => {
            const selectedRow = candidate.user_message_id === selectedId;
            return (
              <button
                key={candidate.user_message_id}
                type="button"
                role="option"
                aria-selected={selectedRow}
                className={`fork-dialog-item${selectedRow ? " fork-dialog-item-selected" : ""}`}
                onClick={() => setSelectedId(candidate.user_message_id)}
              >
                <span className="fork-dialog-item-turn">轮次 {candidate.turn_ordinal}</span>
                <span className="fork-dialog-item-user">{candidate.user_excerpt}</span>
                {candidate.reply_excerpt && (
                  <span className="fork-dialog-item-reply">{candidate.reply_excerpt}</span>
                )}
              </button>
            );
          })}
        </div>

        {error && (
          <div className="fork-dialog-error" role="alert">
            {error}
          </div>
        )}

        <div className="fork-dialog-footer">
          <button
            type="button"
            className="fork-dialog-btn"
            disabled={!selectedId || busyMode !== null}
            onClick={() => void run("workspace")}
          >
            <GitFork size={15} strokeWidth={2} aria-hidden="true" />
            {busyMode === "workspace" ? "正在创建分支…" : "在此工作空间中创建分支"}
          </button>
          <button
            type="button"
            className="fork-dialog-btn fork-dialog-btn-worktree"
            disabled={!selectedId || busyMode !== null || !worktreeSupported}
            title={worktreeSupported ? undefined : "当前 agent 暂不支持在新工作树中分叉"}
            onClick={() => void run("worktree")}
          >
            <GitFork size={15} strokeWidth={2} aria-hidden="true" />
            {busyMode === "worktree" ? "正在创建分支…" : "在新工作树中创建分支"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}