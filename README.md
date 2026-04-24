# Remote Desktop — Rust + Tauri + gRPC

跨平台远程桌面应用。全栈 Rust 实现，采用 gRPC 双向流作为传输协议，支持中转模式。  
屏幕传输使用 **Dirty Rectangle + JPEG** 增量编码，静止画面带宽接近零。

---

## 架构

```
┌─────────────────────────────────────────────────────┐
│  apps/server  (gRPC :50051)                         │
│  • 设备注册   • 会话中转   • 密码验证               │
└──────────────────┬──────────────────────────────────┘
                   │  gRPC 双向流
        ┌──────────┴──────────┐
        │                     │
┌───────▼───────┐     ┌───────▼───────┐
│  HOST 端      │     │  CLIENT 端    │
│  Tauri App    │     │  Tauri App    │
│  • 屏幕采集   │     │  • Canvas 渲染│
│  • JPEG 编码  │     │  • 输入捕获   │
│  • 脏块检测   │     │               │
└───────────────┘     └───────────────┘
```

### 数据流

| 通道 | 方向 | 内容 |
|------|------|------|
| gRPC `HostSession` | HOST → Server → CLIENT | JPEG 帧（Keyframe / Dirty Tiles） |
| gRPC `ClientSession` | CLIENT → Server → HOST | 鼠标/键盘/剪贴板事件 |

---

## 仓库结构

```
remote-desktop/
├── crates/
│   └── proto/              # Protobuf 定义 + 生成代码
├── apps/
│   ├── server/             # 统一 gRPC 服务器（注册 + 中转）
│   │   └── Dockerfile
│   └── desktop/            # Tauri 2 + React + TypeScript 桌面应用
│       ├── src/            # React 前端 (Remote.tsx, Session.tsx, Settings.tsx)
│       └── src-tauri/      # Rust 后端 (capture, relay_client, input, state)
├── docker-compose.yml      # 一键部署服务端
└── .env.example
```

---

## 本地运行

### 前提条件

| 工具 | 版本 | 安装 |
|------|------|------|
| Rust | 1.78+ | `rustup update` |
| Node.js | 18+ | https://nodejs.org |
| protoc | 任意 | 见下方说明 |
| WebView2 | — | Windows 11 已内置；Win10 会自动安装 |

**安装 protoc（Protocol Buffers 编译器）：**

```powershell
# Windows（用 winget）
winget install Google.Protobuf

# macOS
brew install protobuf

# Ubuntu / Debian
sudo apt install protobuf-compiler
```

> macOS 还需要在 **系统设置 → 隐私与安全性 → 屏幕录制** 中授权 desktop 应用。  
> Linux 推荐使用 X11（Wayland 下输入注入支持有限）。

---

### 步骤 1 — 启动 gRPC 服务器

```powershell
cd D:\code\remote-desktop
cargo run -p server -- --host 127.0.0.1 --port 50051
# 输出: INFO listening on 127.0.0.1:50051
```

---

### 步骤 2 — 启动桌面应用

```powershell
cd apps\desktop
npm install          # 首次需要
npm run tauri dev    # 首次编译约 2-3 分钟
```

应用启动后默认连接 `http://localhost:50051`，可在**设置页面**修改。

---

### 步骤 3 — 测试远程连接

**同机测试（HOST + CLIENT 都在本机）：**

1. `npm run tauri dev` 启动的窗口作为 **HOST 端**  
   → 自动开始托管，记下**本设备识别码**（如 `A1B2C3D4`）  
   → 可在验证码框编辑验证码

2. 另开一个终端，运行已编译好的 exe 作为 **CLIENT 端**：
   ```powershell
   .\target\debug\desktop.exe
   ```
   → 在"远程控制设备"区域输入 HOST 的**识别码 + 验证码** → 点击**连接**

3. 连接成功后 CLIENT 窗口显示 HOST 桌面的实时画面，鼠标键盘操作同步传输。

---

## 运行测试

```powershell
# 全部单元测试（服务端 + 桌面端）
cargo test --workspace

# 仅桌面端（含 dirty-rect 算法测试）
cargo test -p desktop
```

期望输出：所有测试通过（约 19 个）。

---

## 生产部署（Docker）

```bash
# 1. 准备环境变量（可选）
cp .env.example .env

# 2. 构建并启动
docker-compose up -d
# gRPC 服务监听 0.0.0.0:50051

# 3. 验证
grpcurl -plaintext your-server:50051 list
```

**桌面应用连接远程服务器：**  
打开**设置页面** → 将"服务器地址"改为 `http://your-server:50051` → 点击**保存**。

---

## 关键依赖

| 功能 | Crate | 版本 |
|------|-------|------|
| gRPC | `tonic` | 0.12 |
| Protobuf | `prost` | 0.13 |
| 屏幕采集 | `xcap` | 0.2 |
| JPEG 编码 | `image` | 0.25 |
| 输入注入 | `enigo` | 0.2 |
| 剪贴板 | `arboard` | 3 |
| 并发注册表 | `dashmap` | 5 |
| 密码哈希 | `argon2` | 0.5 |
| 前端框架 | React 19 + Vite 7 | — |

---

## 屏幕传输算法

采用 **Dirty Rectangle + JPEG**，与 VNC 的 Tight 编码同源：

- 屏幕划分为 128×128 像素的 tile
- 每帧用 FNV-1a hash 比对各 tile，只传输发生变化的 tile
- 静止屏幕 → 0 字节传输；典型桌面操作 → 减少 70-95% 带宽
- 每 200 帧（约 10 秒）强制发送完整 keyframe，防止误差累积
- 网络拥塞时 `try_send` 自动丢弃过期帧，不堆积内存