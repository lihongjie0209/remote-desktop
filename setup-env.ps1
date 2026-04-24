# ============================================================
# setup-env.ps1 — 设置远程桌面项目所需的全局环境变量
# 请以管理员权限运行: PowerShell -ExecutionPolicy Bypass -File setup-env.ps1
# ============================================================

$ErrorActionPreference = "Stop"

function Set-MachineEnv($name, $value) {
    [Environment]::SetEnvironmentVariable($name, $value, "Machine")
    # 同时更新当前进程，无需重启终端即可生效
    [System.Environment]::SetEnvironmentVariable($name, $value, "Process")
    Write-Host "  [SET] $name = $value" -ForegroundColor Green
}

function Add-ToMachinePath($dir) {
    $current = [Environment]::GetEnvironmentVariable("PATH", "Machine")
    if (($current -split ";") -contains $dir) {
        Write-Host "  [SKIP] PATH already contains: $dir" -ForegroundColor Yellow
        return
    }
    $newPath = "$current;$dir"
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "Machine")
    $env:PATH = "$env:PATH;$dir"
    Write-Host "  [ADD]  PATH += $dir" -ForegroundColor Green
}

# ── 检查管理员权限 ─────────────────────────────────────────
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltinRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "ERROR: 请以管理员权限运行此脚本" -ForegroundColor Red
    Write-Host "  PowerShell -ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit 1
}

Write-Host ""
Write-Host "=== 设置系统环境变量 ===" -ForegroundColor Cyan
Write-Host ""

# ── 1. NASM (libjpeg-turbo SIMD 汇编编译器) ───────────────
$nasmDir = "C:\nasm\nasm-2.16.03"
if (Test-Path "$nasmDir\nasm.exe") {
    Add-ToMachinePath $nasmDir
} else {
    Write-Host "  [WARN] NASM 不在 $nasmDir，跳过 PATH 配置" -ForegroundColor Yellow
    Write-Host "         请先安装: https://www.nasm.us/pub/nasm/releasebuilds/2.16.03/win64/nasm-2.16.03-win64.zip"
}

# ── 2. CMake 4.0+ 兼容性 ──────────────────────────────────
# turbojpeg-sys 捆绑的 libjpeg-turbo 使用旧版 CMakeLists.txt
# CMake 4.0 要求 cmake_minimum_required >= 3.5，此变量绕过该检查
Set-MachineEnv "CMAKE_POLICY_VERSION_MINIMUM" "3.5"

# ── 3. libvpx (vcpkg 静态库，用于 WebRTC/VP8 编码) ────────
$vcpkgVpx = "C:\vcpkg\packages\libvpx_x64-windows-static"
if (Test-Path $vcpkgVpx) {
    Set-MachineEnv "VPX_LIB_DIR"       "$vcpkgVpx\lib"
    Set-MachineEnv "VPX_INCLUDE_DIR"   "$vcpkgVpx\include"
    Set-MachineEnv "VPX_STATIC"        "1"
    Set-MachineEnv "VPX_NO_PKG_CONFIG" "1"
    Set-MachineEnv "VPX_VERSION"       "1.13.0"
} else {
    Write-Host "  [SKIP] vcpkg libvpx 路径不存在: $vcpkgVpx" -ForegroundColor Yellow
    Write-Host "         如需 VP8 支持，请先运行: vcpkg install libvpx:x64-windows-static"
}

Write-Host ""
Write-Host "=== 完成! ===" -ForegroundColor Cyan
Write-Host "环境变量已写入系统注册表，新开的终端/进程即可生效。"
Write-Host "当前 PowerShell 会话已同步更新，无需重启。"
Write-Host ""
