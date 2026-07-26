import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GlobalChrome } from "./GlobalChrome";
import type { WorkspaceDescriptor } from "../../types";

vi.mock("./WindowControls", () => ({
  WindowControls: () => null,
}));

const localWorkspace: WorkspaceDescriptor = {
  id: "local",
  name: "kodex",
  root: "D:\\work\\kodex",
  location: { kind: "local" },
};

describe("GlobalChrome", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("keeps shell toggles and omits project-scoped search/terminal controls", () => {
    const onToggleSidebar = vi.fn();
    const onToggleRightPanel = vi.fn();

    render(
      <GlobalChrome
        workspace={localWorkspace}
        sidebarCollapsed={false}
        rightPanelCollapsed={false}
        onToggleSidebar={onToggleSidebar}
        onToggleRightPanel={onToggleRightPanel}
      />,
    );

    expect(screen.queryByRole("button", { name: "搜索工作区" })).toBeNull();
    expect(screen.queryByRole("button", { name: "打开终端" })).toBeNull();
    expect(screen.queryByRole("button", { name: "打开远程终端" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "隐藏项目栏" }));
    fireEvent.click(screen.getByRole("button", { name: "隐藏右侧栏" }));

    expect(onToggleSidebar).toHaveBeenCalledOnce();
    expect(onToggleRightPanel).toHaveBeenCalledOnce();
  });
});
