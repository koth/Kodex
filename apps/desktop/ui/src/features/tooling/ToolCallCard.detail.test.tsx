import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, waitFor } from "@testing-library/react";
import type { ToolInvocation } from "../../types";
import { sessionGetToolDetail } from "../../lib/tauri";
import { ToolCallCard, clearToolDetailCacheForTests } from "./ToolCallCard";

vi.mock("../../lib/tauri", () => ({
  sessionGetToolDetail: vi.fn(() => Promise.reject(new Error("no mock"))),
}));

const mockedGetToolDetail = vi.mocked(sessionGetToolDetail);

function makeTool(overrides: Partial<ToolInvocation> = {}): ToolInvocation {
  return {
    id: "tool-1",
    call_id: "call-1",
    parent_call_id: null,
    name: "Read",
    kind: "read",
    summary: "Read file",
    status: "Succeeded",
    is_subagent: false,
    detail_text: "",
    logs: [],
    diff_paths: [],
    diff_previews: [],
    raw_input: null,
    raw_output: null,
    terminal_output: null,
    error: null,
    permission_options: [],
    permission_input: null,
    permission_decision: null,
    can_stop: false,
    stop_kind: null,
    stop_status: null,
    ...overrides,
  };
}

function renderCard(tool: ToolInvocation) {
  return render(
    <ToolCallCard tool={tool} nested={false} onPermissionSelect={() => {}} />,
  );
}

function expandCard(container: HTMLElement) {
  const header = container.querySelector<HTMLButtonElement>(".tc-header-line");
  expect(header).not.toBeNull();
  fireEvent.click(header!);
}

describe("ToolCallCard – lazy tool detail", () => {
  beforeEach(() => {
    clearToolDetailCacheForTests();
    mockedGetToolDetail.mockReset();
  });

  it("fetches the uncapped detail on first expand when raw output is capped", async () => {
    // The snapshot caps raw_output at 8K chars; a value at the cap signals
    // truncation, so expanding should fetch the full stored detail.
    const cappedOutput = "x".repeat(8 * 1024);
    const tool = makeTool({ raw_output: cappedOutput });
    const fullOutput = `${cappedOutput}TAIL`;
    mockedGetToolDetail.mockResolvedValue(makeTool({ raw_output: fullOutput }));

    const { container } = renderCard(tool);
    expandCard(container);

    await waitFor(() => {
      expect(mockedGetToolDetail).toHaveBeenCalledWith("tool-1");
    });
    await waitFor(() => {
      expect(container.textContent).toContain("TAIL");
    });
  });

  it("does not fetch when the snapshot copy is not capped", () => {
    const tool = makeTool({ raw_output: "small output that fits in the snapshot" });

    const { container } = renderCard(tool);
    expandCard(container);

    expect(mockedGetToolDetail).not.toHaveBeenCalled();
  });

  it("serves repeated expands from the cache without refetching", async () => {
    const cappedOutput = "y".repeat(8 * 1024);
    const tool = makeTool({ raw_output: cappedOutput });
    mockedGetToolDetail.mockResolvedValue(makeTool({ raw_output: `${cappedOutput}END` }));

    const { container } = renderCard(tool);
    expandCard(container);
    await waitFor(() => {
      expect(container.textContent).toContain("END");
    });

    // Collapse and re-expand: the cached detail must render without a
    // second backend call.
    expandCard(container);
    expandCard(container);
    await waitFor(() => {
      expect(container.textContent).toContain("END");
    });
    expect(mockedGetToolDetail).toHaveBeenCalledTimes(1);
  });

  it("keeps the capped snapshot copy when the detail fetch fails", async () => {
    const cappedOutput = "z".repeat(8 * 1024);
    const tool = makeTool({ raw_output: cappedOutput });
    mockedGetToolDetail.mockRejectedValue(new Error("session not found"));

    const { container } = renderCard(tool);
    expandCard(container);

    await waitFor(() => {
      expect(mockedGetToolDetail).toHaveBeenCalledWith("tool-1");
    });
    // The capped snapshot copy still renders; no crash, no empty panel.
    expect(container.textContent).toContain("zzzz");
  });
});
