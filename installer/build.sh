#!/usr/bin/env bash
# =============================================================================
# 构建安装器：主程序 → payload → 编译安装器 → 交付一个中文名的 setup.exe
#
# 用法：
#   bash installer/build.sh            # 用已有的 release 主程序
#   bash installer/build.sh --app      # 先重编主程序，再打安装器
#   bash installer/build.sh --msvc     # 用 MSVC 目标编（单文件，不需要 WebView2Loader.dll）
#   bash installer/build.sh --app --msvc
#
# --msvc 和默认（GNU）的区别只在一件事上：GNU 目标下 webview2-com-sys 只能
# 动态链 WebView2Loader.dll（见该 crate 的 src/lib.rs:14-22），产物必须和那个
# 160 KB 的 dll 放同一目录才能启动；msvc 目标走静态分支，exe 自己就够了。
# 工具链从哪来（不需要装 Visual Studio、不需要管理员）见 installer/msvc-env.sh。
#
# 产物：
#   installer/dist/牛马薪动了_<版本>_x64-setup.exe
#
# 为什么要「主程序先落到 payload 再编」：payload 是编译期压进安装器里的
# （build.rs 用 LZMA 压，见那里的注释），不先更新它，装出去的还是上一版 exe
# ——而且 cargo 不会替你发现，除非 rerun-if-changed 指的文件时间真的变了。
#
# 这个脚本是 CI（.github/workflows/release.yml）和本机共用的一份，所以：
#   · 不写死任何本机路径（以前那行 /c/Users/zorol/... 只有一台机器跑得通）
#   · 缺 node_modules 时自己装
#   · 版本号不一致时直接失败，不带着错版本发版
# =============================================================================
set -euo pipefail

APP_BUILD=0
USE_MSVC=0
for a in "$@"; do
  case "$a" in
    --app) APP_BUILD=1 ;;
    --msvc) USE_MSVC=1 ;;
    -h|--help) sed -n '2,30p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "不认识的参数：$a（用法见 --help）" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="$ROOT/installer"
APP="$ROOT/app"
TAURI_DIR="$APP/src-tauri"

# ---------------------------------------------------------------- 环境
# cargo/rustc 不一定在 PATH 里（Git Bash 下很常见）。CI 上是 msvc，什么都不用加。
for d in "${CARGO_HOME:-$HOME/.cargo}/bin"; do
  [ -d "$d" ] && case ":$PATH:" in *":$d:"*) ;; *) export PATH="$d:$PATH" ;; esac
done

# GNU 工具链的 as / dlltool / windres。windows-gnu 编 tauri-winres 时非有不可，
# 而它们不在 rustup 的 toolchain 里（是 msys2 的 GPL 二进制，见 README 第三节）。
#
# 不写死绝对路径——这脚本 CI 也在跑。从仓库根往上找几层「哪个 app/tools/genv/bin
# 里有 windres」：仓里有一份就用，没有就看工作区上层（这个仓是从大目录里拆出来的，
# 拆之前那份 genv 还在隔壁）。都没有就不加，只在真需要时报一句提示——
# CI 用 msvc，本来一个都不需要。
find_genv() {
  local d="$1"
  for _ in 1 2 3 4; do
    for n in windres.exe windres; do
      [ -x "$d/app/tools/genv/bin/$n" ] && { echo "$d/app/tools/genv/bin"; return; }
    done
    d="$(dirname "$d")"
  done
}

GENV="$(find_genv "$ROOT" || true)"
if [ -n "${GENV:-}" ]; then
  case ":$PATH:" in *":$GENV:"*) ;; *) export PATH="$GENV:$PATH" ;; esac
elif [ "${APP_BUILD:-0}" == "1" ]; then
  echo "提示：PATH 里没有 windres。用 msvc 工具链可以忽略；用 windows-gnu 的话，" >&2
  echo "      按 README 第三节把 app/tools/genv/bin 备齐，否则编到 tauri-winres 会失败。" >&2
fi

# ---------------------------------------------------------------- 版本
# 读版本号要个 JSON 解析器。Windows 的 Git Bash 里 python 可能在、可能叫 python3，
# CI 上则是 setup-python 装的那个——三处都试一遍，都找不到再报错。
PY_BIN="$(command -v python || command -v python3 || command -v py || true)"
if [ -z "$PY_BIN" ]; then
  echo "这台机器上找不到 python，读不了版本号（installer/build.sh 需要它）。" >&2
  exit 1
fi

# 下面几个内联脚本会打印中文（「压缩 7.77 MB → 1.97 MB」）。CI 的 runner 是
# 英文 Windows，python 的 stdout 默认 cp1252，打印中文直接
# UnicodeEncodeError 把构建打挂——本机（中文 Windows / Git Bash）看不出来。
export PYTHONIOENCODING=utf-8

read_ver() { "$PY_BIN" - "$1" <<'PY'
import json, re, sys
p = sys.argv[1]
if p.endswith(".json"):
    print(json.load(open(p, encoding="utf-8"))["version"])
else:
    print(re.search(r'^version = "([^"]+)"', open(p, encoding="utf-8").read(), re.M).group(1))
PY
}

APP_VER="$(read_ver "$TAURI_DIR/tauri.conf.json")"
INS_VER="$(read_ver "$INSTALLER/Cargo.toml")"
CONF_VER="$(read_ver "$INSTALLER/tauri.conf.json")"

if [ "$APP_VER" != "$INS_VER" ] || [ "$APP_VER" != "$CONF_VER" ]; then
  cat >&2 <<EOF
版本号不一致，先对齐再发版：
  主程序      $APP_VER   (app/src-tauri/tauri.conf.json)
  安装器 crate $INS_VER   (installer/Cargo.toml)
  安装器配置   $CONF_VER   (installer/tauri.conf.json)
EOF
  exit 1
fi
VER="$APP_VER"

# CI 打 tag 发版时，tag 和版本必须是同一个数——否则 Release 上挂着 v0.1.8 的
# 标题、里面装的是 0.1.7，manifest 还会按 tag 去找 0.1.8 的包。
#
# 只在 tag 构建时比。PR 和 push 到 master 时 GITHUB_REF_NAME 是 "master"，
# 拿它跟版本号比会把每一次普通构建都判成错——本意是「tag 与版本必须一致」，
# 不是「所有构建都得打 tag」。
if [ "${GITHUB_REF_TYPE:-}" == "tag" ] && [ "${GITHUB_REF_NAME}" != "v$VER" ]; then
  echo "tag ${GITHUB_REF_NAME} 和版本 v$VER 对不上，拒绝构建。" >&2
  exit 1
fi

# ---------------------------------------------------------------- 目标
# GNU（默认）：webview2-com-sys 只能动态链，产物必须和 WebView2Loader.dll 同目录。
# MSVC：走静态分支，exe 自带 loader，单文件（工具链见 msvc-env.sh）。
if [ "$USE_MSVC" == "1" ]; then
  # shellcheck source=installer/msvc-env.sh
  source "$INSTALLER/msvc-env.sh"
  TRIPLE="x86_64-pc-windows-msvc"
  APP_EXE="$TAURI_DIR/target/$TRIPLE/release/paypulse.exe"
  INS_EXE="$INSTALLER/target/$TRIPLE/release/paypulse-setup.exe"
  echo "== 目标 $TRIPLE（单文件，不带 dll）=="
else
  TRIPLE=""
  APP_EXE="$TAURI_DIR/target/release/paypulse.exe"
  INS_EXE="$INSTALLER/target/release/paypulse-setup.exe"
  echo "== 目标 x86_64-pc-windows-gnu（产物要伴生 WebView2Loader.dll）=="
fi

# ---------------------------------------------------------------- 1 主程序
if [ "$APP_BUILD" == "1" ]; then
  echo "== 1/3 编译主程序 =="
  if [ -n "$TRIPLE" ]; then
    # msvc 这条路不需要 tauri CLI：这个工程没有 beforeBuildCommand，
    # 前端就是 ../ui 下的静态文件，cargo 直接编出来的和 npx tauri build
    # 编出来的是同一个东西（少一层 node 依赖，CI 上也不装 node_modules）。
    ( cd "$TAURI_DIR" && cargo build --release --target "$TRIPLE" )
    # cargo 按 crate 名出 paypulse-app.exe；tauri CLI 会把它改名成配置里的
    # mainBinaryName（paypulse.exe），安装器按这个名字写文件（main.rs 的
    # MAIN_EXE）。这里补上 CLI 那一步，不然装出来的目录里没有 paypulse.exe。
    APP_RAW="$TAURI_DIR/target/$TRIPLE/release/paypulse-app.exe"
    if [ -f "$APP_RAW" ]; then
      cp -f "$APP_RAW" "$APP_EXE"
      echo "  产物 $APP_RAW → $(basename "$APP_EXE")"
    fi
  else
    if [ ! -d "$APP/node_modules" ]; then
      ( cd "$APP" && npm install )
    fi
    # 用仓库里锁定的 CLI（app/package.json 的 devDependency）而不是 @latest：
    # 发版包不该因为上游今天出了个新版就换个编译器。
    ( cd "$APP" && npx --no-install tauri build --no-bundle )
  fi
else
  echo "== 1/3 跳过主程序编译（--app 才会重编）=="
fi

# ---------------------------------------------------------------- 2 payload
echo "== 2/3 更新 payload（v$VER）=="
if [ ! -f "$APP_EXE" ]; then
  echo "找不到 $APP_EXE，先跑 bash installer/build.sh --app ${TRIPLE:+--msvc}" >&2
  exit 1
fi
mkdir -p "$INSTALLER/payload"
cp "$APP_EXE" "$INSTALLER/payload/paypulse.exe"
ls -lh "$INSTALLER/payload/paypulse.exe" | awk '{print "  主程序 " $5}'

# 压一遍再塞进安装器：PE 用 LZMA 能到三成四，5.3 MB → 1.8 MB。
# 用 python 而不是 lzma-rs：后者是个老实但很菜的贪心编码器，同一个文件只能压到
# 64%，等于白留 1.6 MB 在安装包里。解压仍然是 lzma-rs（见 main.rs 的 unpack），
# 所以安装器本身不用背一个压缩器。
"$PY_BIN" - "$INSTALLER/payload/paypulse.exe" "$INSTALLER/payload/paypulse.lzma" <<'PY'
import lzma, sys
src, dst = sys.argv[1], sys.argv[2]
raw = open(src, "rb").read()
# preset 6 = 8 MB 字典。和 preset 9 压出来一样大（这个文件不吃字典），
# 但安装时解压要按声明的字典大小分配内存——8 MB 比 64 MB 体面得多。
packed = lzma.compress(raw, format=lzma.FORMAT_ALONE, preset=6)
open(dst, "wb").write(packed)
print(f"  压缩 {len(raw)/1048576:.2f} MB → {len(packed)/1048576:.2f} MB"
      f"（{100*len(packed)/len(raw):.0f}%）")
PY

# ---------------------------------------------------------------- 3 安装器
echo "== 3/3 编译安装器 =="
( cd "$INSTALLER" && cargo build --release $([ -n "$TRIPLE" ] && echo --target "$TRIPLE") )

mkdir -p "$INSTALLER/dist"
OUT="$INSTALLER/dist/牛马薪动了_${VER}_x64-setup.exe"
PREV=""
[ -f "$OUT" ] && PREV="$(stat -c %s "$OUT" 2>/dev/null || echo 0)"
cp "$INS_EXE" "$OUT"

so="$INSTALLER/target${TRIPLE:+/$TRIPLE}/release"

if [ -n "$TRIPLE" ]; then
  # msvc 构建：loader 是静态链进去的，dist 里只能有一个文件。
  # 「有没有漏掉 dll」不能靠 grep 字符串——静态 loader 的代码里本身就带着
  # "WebView2Loader.dll" 这个名字（报错文案用），一搜就有，纯属误报。
  # 真正要看的是 PE 导入表。
  rm -f "$INSTALLER/dist/WebView2Loader.dll"
  READOBJ="$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-readobj.exe 2>/dev/null | head -1)"
  if [ -n "$READOBJ" ]; then
    if "$READOBJ" --coff-imports "$OUT" 2>/dev/null | grep -qi 'WebView2Loader'; then
      echo "构建失败：$OUT 的导入表里还有 WebView2Loader.dll，静态链接没生效。" >&2
      exit 1
    fi
    echo "  单文件：导入表里没有 WebView2Loader.dll（静态 loader 已链入）"
  else
    echo "  提示：没有 llvm-readobj，跳过导入表校验" >&2
  fi
else
  # GNU 工具链编出来的 exe 旁边必须有 WebView2Loader.dll（msvc 静态链，没这个文件）。
  # 它得跟着 setup 一起发：少了它，双击就是一个「找不到 WebView2Loader.dll」的错误框，
  # 而且连界面都出不来。主程序那侧由安装器自己拷进安装目录，见 main.rs 的 copy_webview_loader。
  DLL="$so/WebView2Loader.dll"
  if [ -f "$DLL" ]; then
    cp "$DLL" "$INSTALLER/dist/"
    echo "  附带 WebView2Loader.dll（GNU 构建必需；要单文件请用 --msvc）"
  fi
fi

"$PY_BIN" - "$OUT" "${PREV:-0}" <<'PY'
import os, sys
new = os.path.getsize(sys.argv[1]); prev = int(sys.argv[2] or 0)
mb = lambda n: f"{n/1048576:.1f} MB"
line = f"安装包 {mb(new)}"
if prev > 0:
    line += f"（上一版 {mb(prev)}，{100*(new-prev)/prev:+.0f}%）"
print("\n" + line)
PY
echo "产物：$OUT"
