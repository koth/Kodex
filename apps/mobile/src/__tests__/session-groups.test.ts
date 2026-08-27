import { describe, it, expect } from "vitest";
import { splitSessionGroups } from "../features/session-list/session-groups";
import type { SessionListItem, WorkspaceSessionList } from "../types";

function makeGroup(
  root: string,
  kind: "project" | "chats" | undefined,
  sessions: SessionListItem[] = [],
): WorkspaceSessionList {
  return {
    workspace: {
      id: `id-${root}`,
      name: root,
      root,
      kind,
    },
    sessions,
    active_session_id: "",
    is_active: false,
    connected: true,
  };
}

describe("splitSessionGroups", () => {
  it("routes kind=chats workspaces to the chats bucket and keeps others as projects", () => {
    const { chats, projects } = splitSessionGroups([
      makeGroup("/home/u/proj-a", "project"),
      makeGroup("/home/u/.kodex/chats", "chats"),
      makeGroup("/home/u/proj-b", "project"),
    ]);

    expect(chats.map((g) => g.workspace.root)).toEqual(["/home/u/.kodex/chats"]);
    expect(projects.map((g) => g.workspace.root)).toEqual([
      "/home/u/proj-a",
      "/home/u/proj-b",
    ]);
  });

  it("treats groups without a kind (older desktops) as projects", () => {
    const { chats, projects } = splitSessionGroups([
      makeGroup("/home/u/.kodex/chats", undefined),
      makeGroup("/home/u/proj", undefined),
    ]);

    expect(chats).toEqual([]);
    expect(projects.map((g) => g.workspace.root)).toEqual([
      "/home/u/.kodex/chats",
      "/home/u/proj",
    ]);
  });

  it("preserves session payloads when splitting", () => {
    const session: SessionListItem = {
      id: "s1",
      title: "hello",
      status: "Idle",
      created_at: "",
      updated_at: "",
      message_count: 1,
    };
    const { chats } = splitSessionGroups([makeGroup("/chats", "chats", [session])]);

    expect(chats).toHaveLength(1);
    expect(chats[0].sessions[0].title).toBe("hello");
  });
});