import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommitDialog } from "./CommitDialog";
import {
  gitCommit,
  gitCommitAndPush,
  gitGenerateCommitMessage,
  gitPush,
} from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  gitCommit: vi.fn(),
  gitCommitAndPush: vi.fn(),
  gitGenerateCommitMessage: vi.fn(),
  gitPush: vi.fn(),
}));

vi.mock("../../lib/events", () => ({
  onCommitProgress: vi.fn(async () => () => {}),
}));

describe("CommitDialog", () => {
  beforeEach(() => {
    vi.mocked(gitCommit).mockReset().mockResolvedValue(undefined);
    vi.mocked(gitCommitAndPush).mockReset().mockResolvedValue("pushed");
    vi.mocked(gitGenerateCommitMessage).mockReset().mockResolvedValue("feat: draft");
    vi.mocked(gitPush).mockReset().mockResolvedValue("pushed");
  });

  afterEach(() => {
    cleanup();
  });

  it("commits and pushes staged changes from the primary action", async () => {
    const onClose = vi.fn();
    const onCommitted = vi.fn().mockResolvedValue(undefined);

    render(
      <CommitDialog
        stagedCount={2}
        unstagedCount={1}
        aheadCount={0}
        onClose={onClose}
        onCommitted={onCommitted}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("feat: 简要描述本次改动"), {
      target: { value: "feat: ship it" },
    });
    fireEvent.click(screen.getByRole("button", { name: "提交并推送" }));

    await waitFor(() => {
      expect(gitCommitAndPush).toHaveBeenCalledWith("feat: ship it");
    });
    expect(onCommitted).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("supports push-only when there are unpushed commits and nothing staged", async () => {
    const onClose = vi.fn();
    const onCommitted = vi.fn().mockResolvedValue(undefined);

    render(
      <CommitDialog
        stagedCount={0}
        unstagedCount={0}
        aheadCount={3}
        onClose={onClose}
        onCommitted={onCommitted}
      />,
    );

    expect(screen.getByRole("dialog", { name: "推送" })).toBeTruthy();
    expect(screen.queryByPlaceholderText("feat: 简要描述本次改动")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "推送" }));

    await waitFor(() => {
      expect(gitPush).toHaveBeenCalled();
    });
    expect(onCommitted).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps commit disabled when nothing is staged", () => {
    render(
      <CommitDialog
        stagedCount={0}
        unstagedCount={4}
        aheadCount={0}
        onClose={() => {}}
        onCommitted={() => {}}
      />,
    );

    expect(screen.getByText(/没有已暂存文件/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "提交并推送" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "仅提交" })).toBeDisabled();
  });
});
