import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";

const STORAGE_DEVICE_ID = "rdsk_device_id";
const STORAGE_SERVER_URL = "rdsk_server_url";

export default function Settings() {
  const navigate = useNavigate();

  const [serverUrl, setServerUrl] = useState(
    () => (localStorage.getItem(STORAGE_SERVER_URL) ?? "http://127.0.0.1:50055")
      .replace(/localhost/g, "127.0.0.1")
  );
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved">("idle");
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const [resetStep, setResetStep] = useState(0);

  useEffect(() => {
    invoke<string>("get_or_create_device_id")
      .then(setDeviceId)
      .catch(() => {});
  }, []);

  useEffect(() => {
    invoke("set_server_url", { url: serverUrl }).catch(() => {});
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function saveServerUrl() {
    setSaveState("saving");
    localStorage.setItem(STORAGE_SERVER_URL, serverUrl);
    await invoke("set_server_url", { url: serverUrl }).catch(() => {});
    setSaveState("saved");
    setTimeout(() => setSaveState("idle"), 2000);
  }

  function handleReset() {
    if (resetStep === 0) {
      setResetStep(1);
      setTimeout(() => setResetStep(0), 3500);
    } else {
      localStorage.removeItem(STORAGE_DEVICE_ID);
      setResetStep(0);
      navigate("/", { replace: true });
    }
  }

  function formatDeviceId(id: string) {
    if (id.length === 8) return `${id.slice(0, 4)} · ${id.slice(4)}`;
    return id;
  }

  return (
    <div className="app-layout">
      <aside className="sidebar">
        <div className="sidebar-brand">
          <div className="brand-hex">⬡</div>
          <span className="brand-name">RemoteDesk</span>
          <span className="brand-ver">v0.1</span>
        </div>
        <nav className="sidebar-nav">
          <button className="nav-item" onClick={() => navigate("/")}>
            <span className="nav-icon">⇄</span>
            <span>远程协助</span>
          </button>
          <button className="nav-item active">
            <span className="nav-icon">⚙</span>
            <span>设置</span>
          </button>
        </nav>
        <div className="sidebar-footer">
          <div className="conn-status">
            <span className="conn-dot" />
            安全加密连接
          </div>
        </div>
      </aside>

      <main className="sp-main">
        <header className="sp-header">
          <h1 className="sp-title">设置</h1>
          <p className="sp-subtitle">应用配置与连接参数</p>
        </header>

        <div className="sp-section">
          <div className="sp-section-label">连接</div>
          <div className="sp-card">
            <div className="sp-row">
              <div className="sp-row-meta">
                <div className="sp-row-title">服务器地址</div>
                <div className="sp-row-desc">gRPC 服务器地址（用于中转与信令）</div>
              </div>
              <div className="sp-row-ctrl">
                <input
                  className="sp-input"
                  value={serverUrl}
                  onChange={(e) => setServerUrl(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && saveServerUrl()}
                  spellCheck={false}
                  placeholder="http://127.0.0.1:50055"
                />
                <button
                  className={`sp-btn ${saveState === "saved" ? "sp-btn-saved" : "sp-btn-primary"}`}
                  onClick={saveServerUrl}
                  disabled={saveState === "saving"}
                >
                  {saveState === "saved" ? "✓ 已保存" : saveState === "saving" ? "保存中…" : "保存"}
                </button>
              </div>
            </div>
          </div>
        </div>

        <div className="sp-section">
          <div className="sp-section-label">设备</div>
          <div className="sp-card">
            <div className="sp-row">
              <div className="sp-row-meta">
                <div className="sp-row-title">本机识别码</div>
                <div className="sp-row-desc">与此设备永久绑定，重置后重启将分配新码</div>
              </div>
              <div className="sp-row-ctrl">
                {deviceId ? (
                  <span className="sp-device-id">{formatDeviceId(deviceId)}</span>
                ) : (
                  <span className="sp-device-id sp-device-id--empty">未分配</span>
                )}
                <button
                  className={`sp-btn ${resetStep === 1 ? "sp-btn-danger" : "sp-btn-ghost"}`}
                  onClick={handleReset}
                >
                  {resetStep === 1 ? "⚠ 确认重置" : "重置"}
                </button>
              </div>
            </div>
          </div>
        </div>

        <div className="sp-section">
          <div className="sp-section-label">关于</div>
          <div className="sp-card">
            <div className="sp-row">
              <div className="sp-row-meta">
                <div className="sp-row-title">版本</div>
                <div className="sp-row-desc">当前应用版本</div>
              </div>
              <div className="sp-row-ctrl">
                <span className="sp-version-badge">v0.1.0</span>
              </div>
            </div>
            <div className="sp-row">
              <div className="sp-row-meta">
                <div className="sp-row-title">协议</div>
                <div className="sp-row-desc">gRPC + JPEG 端对端传输</div>
              </div>
              <div className="sp-row-ctrl">
                <span className="sp-proto-badge">gRPC</span>
              </div>
            </div>
          </div>
        </div>
      </main>
    </div>
  );
}