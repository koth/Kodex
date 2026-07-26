import type { WorkspaceDescriptor } from "../../types";
import { isMacOS } from "../../lib/platform";
import { WindowControls } from "./WindowControls";

interface Props {
  workspace: WorkspaceDescriptor;
  sidebarCollapsed: boolean;
  rightPanelCollapsed: boolean;
  onToggleSidebar: () => void;
  onToggleRightPanel: () => void;
}

export function GlobalChrome({
  workspace: _workspace,
  sidebarCollapsed,
  rightPanelCollapsed,
  onToggleSidebar,
  onToggleRightPanel,
}: Props) {
  const chromeClassName = `global-chrome ${isMacOS() ? "is-macos" : ""}`;

  return (
    <header className={chromeClassName} data-tauri-drag-region>
      <div className="global-chrome-left">
        <button
          type="button"
          className={`chrome-icon-btn chrome-sidebar-toggle ${sidebarCollapsed ? "" : "is-active"}`}
          onClick={onToggleSidebar}
          title={sidebarCollapsed ? "显示项目栏" : "隐藏项目栏"}
          aria-label={sidebarCollapsed ? "显示项目栏" : "隐藏项目栏"}
          aria-pressed={!sidebarCollapsed}
        >
          <LeftSidebarIcon />
        </button>
      </div>
      <div className="global-chrome-actions">
        <button
          type="button"
          className={`chrome-icon-btn ${rightPanelCollapsed ? "" : "is-active"}`}
          onClick={onToggleRightPanel}
          title={rightPanelCollapsed ? "显示右侧栏" : "隐藏右侧栏"}
          aria-label={rightPanelCollapsed ? "显示右侧栏" : "隐藏右侧栏"}
          aria-pressed={!rightPanelCollapsed}
        >
          <RightSidebarIcon />
        </button>
        <WindowControls />
      </div>
    </header>
  );
}

function LeftSidebarIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="4" y="5" width="16" height="14" rx="2" />
      <path d="M9 5v14" />
      <path d="M6.5 9h1" />
      <path d="M6.5 12h1" />
      <path d="M6.5 15h1" />
    </svg>
  );
}

function RightSidebarIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="4" y="5" width="16" height="14" rx="2" />
      <path d="M15 5v14" />
      <path d="M17.5 9h-1" />
      <path d="M17.5 12h-1" />
      <path d="M17.5 15h-1" />
    </svg>
  );
}
