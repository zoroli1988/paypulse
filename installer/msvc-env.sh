#!/usr/bin/env bash
# =============================================================================
# 给「没有 Visual Studio、也没有管理员权限」的 Windows 机器准备一套 MSVC 链接环境
#
# 为什么要这么麻烦 —— 单文件安装包只能由 msvc 目标编出来：
#   webview2-com-sys 的源码里写死了两条分支（src/lib.rs:14-22）：
#       target_env = "msvc"  → #[link(name = "WebView2LoaderStatic", kind = "static")]
#       其它（gnu）         → #[link(name = "WebView2Loader.dll")]
#   也就是说 GNU 目标必然在**加载期**依赖 exe 旁边那个 160 KB 的 dll：少一个文件，
#   Windows 加载器直接弹「找不到 WebView2Loader.dll」，我们的代码一行都不会执行
#   （所以没法在程序里兜住）。想做到「发一个文件就能装」，只有 msvc 这一条路。
#
# 而 msvc 目标的链接需要四样东西，本机一样都没有，全部从公开渠道下到用户目录：
#   · rust-std    rustup target add x86_64-pc-windows-msvc        （不需要 VS）
#   · 链接器      rust-lld 复制一份改名 lld-link.exe               （rustup 自带）
#   · SDK 导入库  NuGet: Microsoft.Windows.SDK.CPP.x64             （kernel32.lib 等 482 个）
#   · CRT 静态库  xwin splat（VS 通道里的 MSVC CRT 包）             （libcmt/libcpmt/libvcruntime）
#   · 资源编译器  NuGet: Microsoft.Windows.SDK.BuildTools           （rc.exe，tauri-build 嵌图标要用）
#
# 用静态 CRT（+crt-static）而不是动态：动态要 msvcrt.lib / vcruntime.lib /
# msvcprt.lib，那几个只随 Visual Studio 的 VC 工具集发；静态的那套 xwin 能拿到。
# 顺带的好处是产物连 VC 运行库都不依赖，用户机器上没装过 VS 也能跑。
#
# 本机装没装 VS 都能跑：装了就直接用（一个字节都不下），没装才去准备上面那套。
#
# 用法：
#   source installer/msvc-env.sh          # 首次会下载（约 470 MB），之后离线可用
#   bash   installer/build.sh --msvc      # 正常构建时由 build.sh 负责调用
#
# 缓存目录：$PAYPULSE_MSVC_HOME，默认 %LOCALAPPDATA%\paypulse-msvc
# 想复用已有的 xwin splat：export PAYPULSE_MSVC_CRT_DIR=D:/somewhere/crt/lib/x86_64
# =============================================================================
set -euo pipefail

_msvc_log() { printf '\033[36m[msvc]\033[0m %s\n' "$*" >&2; }
_msvc_die() { printf '\033[31m[msvc] %s\033[0m\n' "$*" >&2; return 1; }

_msvc_home() {
  local d="${PAYPULSE_MSVC_HOME:-}"
  if [ -z "$d" ]; then
    if [ -n "${LOCALAPPDATA:-}" ]; then d="$LOCALAPPDATA/paypulse-msvc"; else d="$HOME/.cache/paypulse-msvc"; fi
  fi
  mkdir -p "$d/bin" "$d/sdk" "$d/crt" "$d/dl"
  printf '%s' "$d"
}

# Git Bash 里 `C:\...` 和 `/c/...` 混着用会把 lld-link 的 /LIBPATH 弄坏，
# 统一转成 lld-link 认的 `C:/...` 形式。
_msvc_win_path() { cygpath -m "$1" 2>/dev/null || printf '%s' "$1"; }

_msvc_have_curl() { command -v curl >/dev/null 2>&1; }

_msvc_download() { # url dest
  local url="$1" dest="$2"
  [ -s "$dest" ] && return 0
  _msvc_log "下载 $(basename "$dest")"
  curl -fL --retry 3 --retry-delay 2 --connect-timeout 20 --max-time 1800 -o "$dest.part" "$url" \
    || { rm -f "$dest.part"; _msvc_die "下载失败：$url"; return 1; }
  mv "$dest.part" "$dest"
}

_msvc_unzip() { # nupkg/zip 目标目录
  local zip="$1" out="$2"
  "$PY_BIN" - "$zip" "$out" <<'PY'
import sys, zipfile
zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])
PY
}

_msvc_bootstrap() {
  local home="$1"

  # ---- 0. python（解压 nupkg 用，和 build.sh 用的是同一个）
  PY_BIN="${PY_BIN:-$(command -v python || command -v python3 || command -v py || true)}"
  [ -n "$PY_BIN" ] || { _msvc_die "找不到 python，解不开 NuGet 包"; return 1; }

  # ---- 1. rust-std (msvc)
  if ! rustup target list --installed 2>/dev/null | grep -qx 'x86_64-pc-windows-msvc'; then
    _msvc_log "安装 rust-std (msvc 目标)"
    rustup target add x86_64-pc-windows-msvc >&2
  fi

  # ---- 2. 链接器：rust-lld 改名 lld-link（lld 按 argv[0] 决定扮演谁）
  if [ ! -x "$home/bin/lld-link.exe" ]; then
    local tc lld
    tc="$(rustc --print sysroot)/lib/rustlib/x86_64-pc-windows-gnu/bin"
    lld="$tc/rust-lld.exe"
    [ -f "$lld" ] || lld="$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/rust-lld.exe 2>/dev/null | head -1)"
    [ -f "$lld" ] || { _msvc_die "找不到 rust-lld.exe"; return 1; }
    cp "$lld" "$home/bin/lld-link.exe"
    _msvc_log "链接器就绪（rust-lld → lld-link）"
  fi

  # ---- 3. rc.exe（tauri-build 嵌图标/版本信息要用）
  if [ ! -x "$home/bin/rc.exe" ]; then
    local v=10.0.26100.9457 pkg="$home/dl/sdkbt.nupkg"
    _msvc_download "https://nuget.azure.cn/v3-flatcontainer/microsoft.windows.sdk.buildtools/$v/microsoft.windows.sdk.buildtools.$v.nupkg" "$pkg" \
      || _msvc_download "https://www.nuget.org/api/v2/package/Microsoft.Windows.SDK.BuildTools/$v" "$pkg"
    _msvc_unzip "$pkg" "$home/dl/sdkbt"
    local rc
    rc="$(find "$home/dl/sdkbt" -path '*/x64/rc.exe' | head -1)"
    [ -n "$rc" ] || { _msvc_die "SDK.BuildTools 里没找到 x64/rc.exe"; return 1; }
    cp "$rc" "$home/bin/rc.exe"
    _msvc_log "rc.exe 就绪"
  fi

  # ---- 4. SDK 导入库（kernel32/user32/ole32/uuid… + ucrt）
  if [ ! -f "$home/sdk/um/x64/kernel32.lib" ]; then
    local v=10.0.26100.9457 pkg="$home/dl/sdkcpp.nupkg"
    _msvc_download "https://nuget.azure.cn/v3-flatcontainer/microsoft.windows.sdk.cpp.x64/$v/microsoft.windows.sdk.cpp.x64.$v.nupkg" "$pkg" \
      || _msvc_download "https://www.nuget.org/api/v2/package/Microsoft.Windows.SDK.CPP.x64/$v" "$pkg"
    _msvc_unzip "$pkg" "$home/dl/sdkcpp"
    # nupkg 里是 c/um/x64/*.lib 和 c/ucrt/x64/*.lib。注意 Windows 的文件名大小写
    # 不敏感、但 tar/zip 里混着 .Lib 和 .lib，两种都拷一遍最省事。
    mkdir -p "$home/sdk/um/x64" "$home/sdk/ucrt/x64"
    cp "$home/dl/sdkcpp/c/um/x64/"*.[lL]ib "$home/sdk/um/x64/" 2>/dev/null || true
    cp "$home/dl/sdkcpp/c/ucrt/x64/"*.[lL]ib "$home/sdk/ucrt/x64/" 2>/dev/null || true
    [ -f "$home/sdk/um/x64/kernel32.lib" ] || { _msvc_die "SDK 导入库解压后没找到 kernel32.lib"; return 1; }
    _msvc_log "SDK 导入库就绪（$(ls "$home/sdk/um/x64" | wc -l) 个）"
  fi

  # ---- 5. CRT 静态库（libcmt / libcpmt / libvcruntime）
  if [ ! -f "$home/crt/libcmt.lib" ]; then
    if [ -n "${PAYPULSE_MSVC_CRT_DIR:-}" ] && [ -f "${PAYPULSE_MSVC_CRT_DIR}/libcmt.lib" ]; then
      cp "${PAYPULSE_MSVC_CRT_DIR}/"*.lib "$home/crt/"
      _msvc_log "CRT 静态库复用 $PAYPULSE_MSVC_CRT_DIR"
    else
      # xwin：微软自家 VS 通道里的包，它负责把 CRT/SDK 拆出来。
      # 只用它的 crt 部分（SDK 导入库上面已经从 NuGet 拿到了，不必再等 1 GB）。
      local xtgz="$home/dl/xwin.tgz" xdir="$home/dl/xwin"
      if [ ! -x "$xdir/xwin.exe" ]; then
        _msvc_download "https://github.com/Jake-Shadle/xwin/releases/download/0.10.0/xwin-0.10.0-x86_64-pc-windows-msvc.tar.gz" "$xtgz"
        mkdir -p "$xdir"
        tar xzf "$xtgz" -C "$xdir" --strip-components=1
      fi
      _msvc_log "用 xwin 拉 MSVC CRT（首次约 100 MB，慢的话可以 Ctrl-C 后重跑）"
      "$xdir/xwin.exe" --accept-license splat --output "$home/dl/xwin-splat" >&2 || true
      local src="$home/dl/xwin-splat/crt/lib/x86_64"
      if [ -f "$src/libcmt.lib" ]; then
        cp "$src/"*.lib "$home/crt/" 2>/dev/null || true
      fi
      [ -f "$home/crt/libcmt.lib" ] || { _msvc_die "没拿到 CRT 静态库（libcmt.lib）"; return 1; }
      _msvc_log "CRT 静态库就绪"
    fi
  fi

  : >"$home/.ready"
}

# ------------------------------------------------------------------ 入口
# 判断本机有没有真正的 Visual Studio。三个探针，从最便宜的往下试：
#   · VSINSTALLDIR —— 只有「开发者命令提示符」里才有值；有就一定对
#   · cl.exe       —— 同上，正规构建机上一般不在 PATH 里
#   · vswhere.exe  —— VS2017 起随安装器固定在 Program Files (x86)，
#                     windows-latest 就是这种情形：VS 装着，但上面两个都没有。
#                     少了这个探针，CI 会白下载 470 MB。
# 不能看 link.exe：Git Bash 里 /usr/bin/link.exe 是 coreutils 的「建硬链接」
# 工具，名字撞车，一查就误判成「本机有 VS」。
_msvc_has_vs() {
  [ -n "${VSINSTALLDIR:-}" ] && return 0
  command -v cl.exe >/dev/null 2>&1 && return 0
  local vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
  [ -x "$vswhere" ] || return 1
  # -products '*' 才看得见 Build Tools（默认只查 VS 本体）；
  # 再要求 x86/x64 的 VC 工具集——装了 VS 却没勾 C++ 也不顶用。
  "$vswhere" -latest -products '*' \
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
    -property installationPath 2>/dev/null | grep -q .
}

if _msvc_has_vs; then
  _msvc_log "检测到本机已有 Visual Studio 工具链，直接用（不下载任何东西）"
  # CRT 仍然静态链，和「本机没 VS」那条路保持一致——否则同一版本会出两种
  # 产物：CI 的依赖 vcruntime140.dll，本机的不依赖，用户装哪一个看运气。
  # 静态 CRT 的库由 VS 自己提供，不需要我们 -L。
  #
  # /ignore:4099 消 LNK4099（见下面那段注释）：VS 里的 libcmt.lib 同样只带
  # 一个指向微软内部 PDB 的路径，一样加载不到。
  export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static \
 -C link-arg=/ignore:4099"
  export PAYPULSE_MSVC_READY=1
  return 0 2>/dev/null || exit 0
fi

MSVC_HOME="$(_msvc_home)"
if [ ! -f "$MSVC_HOME/.ready" ]; then
  _msvc_log "首次使用，准备工具链到 $MSVC_HOME"
  _msvc_bootstrap "$MSVC_HOME"
fi

if ! rustup target list --installed 2>/dev/null | grep -qx 'x86_64-pc-windows-msvc'; then
  _msvc_die "rust-std (msvc) 还没装好"
  return 1 2>/dev/null || exit 1
fi

export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$(_msvc_win_path "$MSVC_HOME/bin/lld-link.exe")"
export RC="$(_msvc_win_path "$MSVC_HOME/bin/rc.exe")"

# 静态 CRT：产物不依赖 VC 运行库，也不依赖 msvcrt.lib 这类只随 VS 发的导入库。
#
# 用 CARGO_TARGET_<triple>_RUSTFLAGS 而不是 RUSTFLAGS：后者会一并套到构建脚本
# 和过程宏上（它们按 host 编，本机是 gnu），给 gnu 的链接行塞一串 msvc 的 -L
# 纯属噪音；在 CI 上 host 就是 msvc，+crt-static 落到 proc-macro 的 dylib 上
# 还有已知的坑。按目标限定的这个变量只作用于 msvc 那一个 target。
#
# /ignore:4099 是消音用的。静态 CRT 的每个 .obj 里都留了一句「我的调试信息在
# D:\a\_work\1\s\binaries\amd64ret\lib\amd64\libcmt.amd64.pdb」——那是微软自己
# 构建机的路径，微软从不发布这个 pdb，所以 lld-link 每次都刷几十条
# LNK4099「Cannot use debug info for libcmt.lib(...)」。它只影响调试符号的可用性，
# 对产物没影响，但足以把真正的链接错误淹掉。
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static \
 -C link-arg=/ignore:4099 \
 -L native=$(_msvc_win_path "$MSVC_HOME/sdk/um/x64") \
 -L native=$(_msvc_win_path "$MSVC_HOME/sdk/ucrt/x64") \
 -L native=$(_msvc_win_path "$MSVC_HOME/crt")"

# tauri-build 通过 embed-resource 找资源编译器。它是按**构建脚本自己的** host
# 选工具的：本机 host 是 windows-gnu，于是它会去找 windres，压根不看 RC。
# 所以 windres 必须留在 PATH 里（build.sh 已经负责把 genv/bin 加进去），
# 它编出来的资源对象 lld-link 同样能吃。
export PAYPULSE_MSVC_READY=1
