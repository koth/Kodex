import type { WorkspaceSessionList } from "../../types";

// Split the `ListSessions` groups into the project-less chats workspace(s)
// (marked `kind: "chats"` by the backend) and regular projects. Mirrors the
// desktop sidebar, which renders the chats workspace as its own first-class
// "聊天" group instead of a project. Groups from desktops that predate the
// `kind` field have no marker and fall into `projects` (previous behavior).
export function splitSessionGroups(groups: WorkspaceSessionList[]): {
  chats: WorkspaceSessionList[];
  projects: WorkspaceSessionList[];
} {
  const chats: WorkspaceSessionList[] = [];
  const projects: WorkspaceSessionList[] = [];
  for (const group of groups) {
    if (group.workspace.kind === "chats") {
      chats.push(group);
    } else {
      projects.push(group);
    }
  }
  return { chats, projects };
}