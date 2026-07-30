import { useEffect, useRef, useState } from "react";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { VRMLoaderPlugin, VRMUtils, type VRM } from "@pixiv/three-vrm";

export type VrmLoadStatus = "loading" | "ready" | "error";

export interface VrmModelResult {
  vrm: VRM | null;
  status: VrmLoadStatus;
  error: string | null;
}

const vrmCache = new Map<string, Promise<VRM>>();

function loadVrm(url: string): Promise<VRM> {
  const cached = vrmCache.get(url);
  if (cached) return cached;
  const promise = new Promise<VRM>((resolve, reject) => {
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMLoaderPlugin(parser));
    loader.load(
      url,
      (gltf) => {
        const vrm = gltf.userData.vrm as VRM | undefined;
        if (!vrm) {
          reject(new Error("文件是 glTF 但不含 VRM 扩展（可能是加密导出或非 VRM 文件）"));
          return;
        }
        VRMUtils.removeUnnecessaryVertices(gltf.scene);
        VRMUtils.combineSkeletons(gltf.scene);
        resolve(vrm);
      },
      undefined,
      (event) => {
        console.error("[companion] VRM 加载失败:", url, event);
        reject(
          event instanceof Error
            ? event
            : new Error(`网络/权限拒绝（asset 协议无法读取该路径）: ${url}`),
        );
      },
    );
  });
  promise.catch(() => vrmCache.delete(url));
  vrmCache.set(url, promise);
  return promise;
}

/** 加载 VRM 模型（含缓存、错误处理）。url 为 null 时返回 error 状态供降级。 */
export function useVrmModel(url: string | null): VrmModelResult {
  const [result, setResult] = useState<VrmModelResult>({
    vrm: null,
    status: url ? "loading" : "error",
    error: url ? null : "未配置模型",
  });
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!url) {
      setResult({ vrm: null, status: "error", error: "未配置模型" });
      return;
    }
    setResult({ vrm: null, status: "loading", error: null });
    let cancelled = false;
    loadVrm(url)
      .then((vrm) => {
        if (!cancelled && mountedRef.current) {
          setResult({ vrm, status: "ready", error: null });
        }
      })
      .catch((err: unknown) => {
        if (!cancelled && mountedRef.current) {
          setResult({
            vrm: null,
            status: "error",
            error: err instanceof Error ? err.message : "VRM 加载失败",
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [url]);

  return result;
}
