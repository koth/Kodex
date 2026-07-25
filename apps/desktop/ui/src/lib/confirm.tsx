import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { AlertTriangle, GitBranch, Trash2 } from "lucide-react";
import {
  ConfirmDialog,
  type ConfirmDialogTone,
} from "../features/changes/ConfirmDialog";

export type ConfirmRequest = {
  title: string;
  description: ReactNode;
  detail?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: ConfirmDialogTone;
  icon?: ReactNode;
};

type ConfirmHostApi = {
  confirm: (request: ConfirmRequest) => Promise<boolean>;
};

type PendingConfirm = ConfirmRequest & {
  id: number;
  resolve: (value: boolean) => void;
};

const ConfirmContext = createContext<ConfirmHostApi | null>(null);

let externalConfirm: ((request: ConfirmRequest) => Promise<boolean>) | null = null;
let confirmSeq = 0;

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<PendingConfirm | null>(null);
  const pendingRef = useRef<PendingConfirm | null>(null);

  const settle = useCallback((value: boolean) => {
    const current = pendingRef.current;
    if (!current) return;
    pendingRef.current = null;
    setPending(null);
    current.resolve(value);
  }, []);

  const confirm = useCallback((request: ConfirmRequest) => {
    return new Promise<boolean>((resolve) => {
      const previous = pendingRef.current;
      if (previous) {
        previous.resolve(false);
      }

      const next: PendingConfirm = {
        ...request,
        id: ++confirmSeq,
        resolve,
      };
      pendingRef.current = next;
      setPending(next);
    });
  }, []);

  const api = useMemo<ConfirmHostApi>(() => ({ confirm }), [confirm]);

  useEffect(() => {
    externalConfirm = confirm;
    return () => {
      if (externalConfirm === confirm) {
        externalConfirm = null;
      }
    };
  }, [confirm]);

  return (
    <ConfirmContext.Provider value={api}>
      {children}
      <ConfirmDialog
        open={pending != null}
        title={pending?.title ?? ""}
        description={pending?.description ?? null}
        detail={pending?.detail}
        confirmLabel={pending?.confirmLabel}
        cancelLabel={pending?.cancelLabel}
        tone={pending?.tone}
        icon={pending?.icon}
        onCancel={() => settle(false)}
        onConfirm={() => settle(true)}
      />
    </ConfirmContext.Provider>
  );
}

export function useConfirm() {
  const ctx = useContext(ConfirmContext);
  if (!ctx) {
    throw new Error("useConfirm must be used within ConfirmProvider");
  }
  return ctx.confirm;
}

export async function appConfirm(request: ConfirmRequest): Promise<boolean> {
  if (!externalConfirm) {
    throw new Error("ConfirmProvider is not mounted");
  }
  return externalConfirm(request);
}

export function trackConfirmRequest(input: {
  path: string;
  count?: number;
}): ConfirmRequest {
  const count = input.count ?? 1;
  if (count > 1) {
    return {
      title: "跟踪文件",
      description: `将跟踪目录下的 ${count} 个未跟踪文件，并加入 Git 索引。`,
      detail: input.path,
      confirmLabel: `跟踪 ${count} 个文件`,
      icon: <GitBranch size={16} strokeWidth={2.1} />,
    };
  }

  return {
    title: "跟踪文件",
    description: "将这个未跟踪文件加入 Git 索引。",
    detail: input.path,
    confirmLabel: "跟踪",
    icon: <GitBranch size={16} strokeWidth={2.1} />,
  };
}

export function rejectPatchConfirmRequest(path: string): ConfirmRequest {
  return {
    title: "撤销改动",
    description: "将丢弃该文件的工作区改动，此操作不可撤销。",
    detail: path,
    confirmLabel: "撤销改动",
    tone: "danger",
    icon: <AlertTriangle size={16} strokeWidth={2.1} />,
  };
}

export function deleteFileConfirmRequest(path: string): ConfirmRequest {
  return {
    title: "删除文件",
    description: "将永久删除该文件，此操作不可撤销。",
    detail: path,
    confirmLabel: "删除",
    tone: "danger",
    icon: <Trash2 size={16} strokeWidth={2.1} />,
  };
}

export function archiveWorkspaceConfirmRequest(label: string): ConfirmRequest {
  return {
    title: "归档项目",
    description: "归档后该项目及其所有会话将从列表中移除，数据仍保留在本地。",
    detail: label,
    confirmLabel: "归档",
    tone: "danger",
    icon: <AlertTriangle size={16} strokeWidth={2.1} />,
  };
}

export function removeProviderConfirmRequest(label: string): ConfirmRequest {
  return {
    title: "移除 Provider",
    description: "此操作会删除 endpoint、模型列表和已保存的 API key。",
    detail: label,
    confirmLabel: "移除",
    tone: "danger",
    icon: <Trash2 size={16} strokeWidth={2.1} />,
  };
}

export function clearProviderConfirmRequest(label: string): ConfirmRequest {
  return {
    title: "清除设置",
    description: "此操作会删除已保存的 API key、模型列表和列表 URL。",
    detail: label,
    confirmLabel: "清除",
    tone: "danger",
    icon: <AlertTriangle size={16} strokeWidth={2.1} />,
  };
}

export function deleteAllArchivedConfirmRequest(): ConfirmRequest {
  return {
    title: "删除已归档对话",
    description: "确定删除所有已归档对话？此操作不可撤销。",
    confirmLabel: "全部删除",
    tone: "danger",
    icon: <Trash2 size={16} strokeWidth={2.1} />,
  };
}
