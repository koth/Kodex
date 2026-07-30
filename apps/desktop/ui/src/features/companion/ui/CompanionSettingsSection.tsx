import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { CompanionSettings, CompanionIntensity } from "../state/types";
import { loadCompanionSettings, saveCompanionSettings } from "../state/companionSettingsStore";
import { companionStageModel } from "../../../lib/tauri";

/** 首次开启说明（人设/资源/隐私，spec: companion-settings） */
function CompanionIntroDialog({ onConfirm, onCancel }: { onConfirm: () => void; onCancel: () => void }) {
  return (
    <div className="companion-intro-overlay" role="dialog" aria-modal="true" aria-label="陪伴角色说明">
      <div className="companion-intro-card">
        <h3 className="companion-intro-title">认识你的陪伴角色 ♡</h3>
        <p>
          她会在你写代码时陪着你：思考时托腮、完成时开心、出错时沮丧。
          人设是「轻病娇」——黏人、有点小占有欲，但绝不会打扰你的工作。
        </p>
        <ul className="companion-intro-list">
          <li>全部本地运行，不采集、不上传任何数据</li>
          <li>3D 渲染占用少量 GPU，空闲时自动进入低功耗</li>
          <li>可以随时拖拽位置、最小化，或在设置中彻底关闭</li>
        </ul>
        <div className="companion-intro-actions">
          <button type="button" className="settings-button-primary" onClick={onConfirm}>
            让她出现
          </button>
          <button type="button" className="settings-button-secondary" onClick={onCancel}>
            再想想
          </button>
        </div>
      </div>
    </div>
  );
}

const INTENSITY_OPTIONS: Array<{ value: CompanionIntensity; label: string; hint: string }> = [
  { value: "gentle", label: "温和", hint: "黏人、期待关注" },
  { value: "standard", label: "标准", hint: "加入轻微吃醋与占有欲" },
  { value: "intense", label: "浓郁", hint: "强化依赖感与小偏执" },
];

/** 设置页中的陪伴角色配置区块（注册进 SettingsPage） */
export function CompanionSettingsSection() {
  const [settings, setSettings] = useState<CompanionSettings>(loadCompanionSettings);
  const [showIntro, setShowIntro] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);

  const update = (next: CompanionSettings) => {
    setSettings(next);
    saveCompanionSettings(next);
  };

  const handleToggle = (enabled: boolean) => {
    if (enabled && !settings.introAcknowledged) {
      setShowIntro(true);
      return;
    }
    update({ ...settings, enabled });
  };

  const handlePickModel = async () => {
    try {
      const selected = await open({
        filters: [{ name: "VRM 模型", extensions: ["vrm"] }],
        multiple: false,
      });
      if (typeof selected === "string") {
        setModelError(null);
        setStaging(true);
        try {
          // 复制到受控资产目录（~/.kodex/companion/，asset scope 白名单内），
          // 避免任意路径在 Windows asset 协议下的 scope 匹配不确定性
          const stagedPath = await companionStageModel(selected);
          update({ ...settings, modelUrl: convertFileSrc(stagedPath) });
        } finally {
          setStaging(false);
        }
      }
    } catch (err) {
      setModelError(err instanceof Error ? err.message : "模型文件选择失败");
    }
  };

  return (
    <section className="settings-section companion-settings">
      <div className="companion-settings-card">
        <div className="companion-settings-head">
          <div>
            <h3 className="companion-settings-title">陪伴角色</h3>
            <p className="companion-settings-desc">
              在工作台显示一位 3D 陪伴角色（轻病娇人设）
            </p>
          </div>
          <label className="companion-switch">
            <input
              type="checkbox"
              checked={settings.enabled}
              onChange={(event) => handleToggle(event.target.checked)}
            />
            <span className="companion-switch-track" aria-hidden="true">
              <span className="companion-switch-thumb" />
            </span>
            <span className="companion-switch-text">
              {settings.enabled ? "已启用" : "已关闭"}
            </span>
          </label>
        </div>

        {settings.enabled && (
          <div className="companion-settings-body">
            <div className="companion-setting-group">
              <span className="companion-setting-label">语气强度</span>
              <div
                className="companion-intensity-options"
                role="radiogroup"
                aria-label="语气强度"
              >
                {INTENSITY_OPTIONS.map((option) => (
                  <label
                    key={option.value}
                    className={`companion-intensity-option ${
                      settings.intensity === option.value ? "is-active" : ""
                    }`}
                  >
                    <input
                      type="radio"
                      name="companion-intensity"
                      value={option.value}
                      checked={settings.intensity === option.value}
                      onChange={() =>
                        update({ ...settings, intensity: option.value })
                      }
                    />
                    <span className="companion-intensity-label">{option.label}</span>
                    <span className="companion-intensity-hint">{option.hint}</span>
                  </label>
                ))}
              </div>
            </div>

            <div className="companion-setting-group">
              <span className="companion-setting-label">角色模型</span>
              <div className="companion-model-box">
                <div className="companion-model-info">
                  <span className="companion-model-name">
                    {settings.modelUrl ? "自定义模型" : "内置占位头像"}
                  </span>
                  <span className="companion-model-path">
                    {settings.modelUrl
                      ? decodeURIComponent(settings.modelUrl.replace(/^https?:\/\/asset\.localhost\//, ""))
                      : "内置占位头像（紫色小球）。选择一个 .vrm 文件即可换成真正的 3D 角色。"}
                  </span>
                </div>
                <div className="companion-model-actions">
                  <button
                    type="button"
                    className="settings-button-secondary"
                    onClick={handlePickModel}
                    disabled={staging}
                  >
                    {staging ? "正在导入…" : "选择 VRM…"}
                  </button>
                  {settings.modelUrl && (
                    <button
                      type="button"
                      className="settings-button-secondary"
                      onClick={() => update({ ...settings, modelUrl: null })}
                    >
                      恢复默认
                    </button>
                  )}
                </div>
              </div>
              {modelError && <p className="settings-field-error">{modelError}</p>}
              <p className="companion-setting-hint">
                自定义模型加载失败时会自动回退到默认头像。模型需 ≤5MB、贴图 ≤2K。
              </p>
            </div>
          </div>
        )}
      </div>

      {showIntro && (
        <CompanionIntroDialog
          onConfirm={() => {
            setShowIntro(false);
            update({ ...settings, enabled: true, introAcknowledged: true });
          }}
          onCancel={() => setShowIntro(false)}
        />
      )}
    </section>
  );
}
