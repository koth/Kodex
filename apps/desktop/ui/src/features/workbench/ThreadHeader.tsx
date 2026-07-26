import type { ReactNode } from "react";
import type { SessionSummary } from "../../types";

interface Props {
  session: SessionSummary;
  actions?: ReactNode;
}

export function ThreadHeader({
  session,
  actions,
}: Props) {
  return (
    <header className="thread-header">
      <div className="thread-header-main">
        <h1 className="thread-header-title" title={session.title}>
          {session.title}
        </h1>
      </div>
      {actions && <div className="thread-header-actions">{actions}</div>}
    </header>
  );
}
