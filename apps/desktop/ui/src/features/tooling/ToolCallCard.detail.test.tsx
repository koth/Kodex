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
  it("fetches the full detail when raw_input carries the _truncated marker", async () => {
    // The backend smart-compacts large raw_input payloads: it keeps
    // priority fields, drops heavy ones (e.g. a create call's `file_text`),
    // and re-serializes with a `"_truncated": true` marker. The compacted
    // payload can be far shorter than the 4K length gate, so the marker is
    // the only signal that the snapshot is incomplete and the full stored
    // record must be pulled on expand to rebuild the diff surface.
    const compactedInput =
      '{"command":"create","path":"/a/b.tsx","_truncated":true}';
    const tool = makeTool({
      name: "str_replace_editor",
      kind: "edit",
      raw_input: compactedInput,
      diff_paths: ["/a/b.tsx"],
    });
    const fullInput =
      '{"command":"create","path":"/a/b.tsx","file_text":"export const X = 1;\\n"}';
    mockedGetToolDetail.mockResolvedValue(
      makeTool({ name: "str_replace_editor", kind: "edit", raw_input: fullInput }),
    );

    const { container } = renderCard(tool);
    expandCard(container);

    await waitFor(() => {
      expect(mockedGetToolDetail).toHaveBeenCalledWith("tool-1");
    });
  });
});
