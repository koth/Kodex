import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

afterEach(() => {
  cleanup();
});

describe("ConfirmDialog", () => {
  it("confirms and cancels from the in-app dialog", () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();

    render(
      <ConfirmDialog
        open
        title="跟踪文件"
        description="将这个未跟踪文件加入 Git 索引。"
        detail="src/file.ts"
        confirmLabel="跟踪"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    expect(screen.getByRole("dialog", { name: "跟踪文件" })).toBeTruthy();
    expect(screen.getByText("src/file.ts")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onCancel).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "跟踪" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});
