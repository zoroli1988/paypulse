# 牛马薪动了！（paypulse）

桌面实时薪资悬浮窗。一个无边框、置顶、可点击穿透的小窗，把「今天已经赚到多少钱」按秒往前推。

> 月薪 8000 ÷ 21.75 计薪日 ÷ 7.5 小时 → 每秒钟 **0.0136 元**。
> 数字不是估的，是算的；而且每一帧都是从当前时刻重算的，不是累加的。

**所有薪资数据只存在你自己的电脑里——不联网、不上传、不需要账号。**

---

## 这个仓库里有什么

```
app/                       外壳（本仓，AGPL-3.0）
paypulse-clock/            时刻 → 已计薪秒的换算（纯函数，AGPL-3.0，带完整单测）
paypulse-core/             [submodule] 计薪引擎（私有仓，不在此仓库，见下方）
paypulse-prototype.html    高保真原型（设计稿，唯一视觉基准）
```

**为什么切开**：外壳开源、引擎闭源。分界线的意义是「21.75 从哪来」这个问题在外壳里翻不到答案——公式、常数、区间封顶全在引擎（`paypulse-core`）里，外壳通过 IPC 只拿到结果 `{ paid, shadow, ot }`。

详细的设计、计薪规则、构建步骤见 [`app/README.md`](app/README.md)。

## 开源许可

- **本仓库（外壳 + 时钟库）**：AGPL-3.0，公开、收 PR。fork 自用、改配色、改成自己的薪资，随便改；但 fork 之后闭源分发（比如加「会员解锁」再上架）不被允许。
- **计薪引擎（paypulse-core）**：闭源、私有，仅以编译后的二进制（`.dll` / `.dylib`）随主程序发行。源码不在本仓库，也不在公开的任何地方。

选 AGPL 而不是 MIT 的原因很直接：这是个桌面应用，别人 fork 之后加上「会员解锁加班统计」再闭源分发是可能的，AGPL 至少要求他把改动放出来。

## 拉取私有引擎（submodule）

主仓通过 git submodule 引用私有引擎。有权限访问 `paypulse-core` 私有仓的人：

```bash
git clone --recurse-submodules git@github.com:zoroli1988/paypulse.git
```

没有权限也没关系——外壳代码、时钟库、原型、文档都能看；只是编译二进制时缺引擎这一步（`app/src-tauri/Cargo.toml` 里 `paypulse-core` 的 `path` 依赖指向 submodule 目录）。

## 快速开始

详见 [`app/README.md`](app/README.md) 的第三节（构建环境、四个坑与解法）。Windows 用 `x86_64-pc-windows-gnu` 工具链，需要补 GNU binutils（`as` / `dlltool` / `windres` / `windmc`），README 已写清从 msys2 获取的步骤。

## 平台支持

- **Windows**：完整实现（悬浮窗、托盘、开机自启、全局快捷键、空闲检测、边缘吸附、穿透）。
- **macOS**：平台层已就位（`platform.rs` 的 `#[cfg(target_os = "macos")]` 分支），开机自启用 LaunchAgent、空闲检测用 CGEventSource、快捷键用 Carbon `RegisterEventHotKey`。**尚未在 macOS 真机验证**——编译与联调依赖 GitHub Actions 的 macOS runner（见 `.github/workflows/ci.yml` 的 `shell-macos`）。快捷键的触发回调（Carbon 事件处理器 + RunLoop 集成）是唯一待接入的部分。
- **Linux**：平台功能退化为空实现（能编译，但开机自启/快捷键/空闲检测/吸附不可用）。

## License

外壳与时钟库：AGPL-3.0，见 [`app/LICENSE`](app/LICENSE)。引擎私有，见私有仓 `paypulse-core`。
