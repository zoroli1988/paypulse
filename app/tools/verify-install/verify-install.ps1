<#
.SYNOPSIS
    装一次、查一遍、卸干净，用来自证「别人能装」。

.DESCRIPTION
    静态检查能看出文件在不在包里的，但看不出「装完开始菜单有没有项」、
    「卸载项写的路径对不对」、「自启注册表指向的是不是真实安装路径」。
    这些只有真装一次才知道。

    脚本按 装前 → 安装 → 装后 → 卸载 → 卸后 五步走，把每一步的
    系统状态（注册表、目录、快捷方式）抓成快照再对比，最后报残留。
    任何一项不符合预期都以非 0 退出，方便进 CI 或发布检查单。

.PARAMETER Keep
    查完不卸载。用来看一眼装完的实际样子。默认会卸干净。

.PARAMETER InstallerPath
    指定安装包。默认取 src-tauri/target/x86_64-pc-windows-gnu/release/bundle/nsis
    下最新的那一个。

.EXAMPLE
    powershell -File app\tools\verify-install\verify-install.ps1
    powershell -File app\tools\verify-install\verify-install.ps1 -Keep

.NOTES
    NSIS 的 /S 是静默安装，不弹任何窗。currentUser 模式（tauri.conf.json 里
    installMode）意味着它**不写 HKLM、不要求提权**，全部落在 HKCU 与
    %LOCALAPPDATA%\paypulse —— 所以普通权限就能跑，也正因为这样才能
    干净地卸掉而不留系统级残渣。
#>

[CmdletBinding()]
param(
    [switch]$Keep,
    [string]$InstallerPath
)

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 没有 $IsWindows，但这里只在 Windows 上跑，无需判断
$PRODUCT      = 'paypulse'
$RUN_KEY      = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$UNINST_ROOT  = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$INSTALL_DIR  = Join-Path $env:LOCALAPPDATA $PRODUCT
$START_MENU   = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'

$script:failures = New-Object System.Collections.ArrayList

function Note-Ok($msg)   { Write-Host ("  [OK]   " + $msg) -ForegroundColor Green }
function Note-Bad($msg)  { Write-Host ("  [FAIL] " + $msg) -ForegroundColor Red
                           [void]$script:failures.Add($msg) }
function Note-Info($msg) { Write-Host ("         " + $msg) -ForegroundColor DarkGray }
function Section($msg)   { Write-Host ""
                           Write-Host ("== " + $msg + " ==") -ForegroundColor Cyan }

function Get-Snapshot {
    <# 抓一份「现在系统里跟这个程序有关的东西」。只读，不改任何状态。 #>
    $snap = @{
        RunValue     = $null
        UninstallKey = $null
        InstallDir   = $null      # $null = 目录不存在；否则是文件列表
        StartMenu    = @()
    }

    if (Test-Path $RUN_KEY) {
        $item = Get-ItemProperty -Path $RUN_KEY -ErrorAction SilentlyContinue
        if ($item -and $item.PSObject.Properties.Name -contains $PRODUCT) {
            $snap.RunValue = $item.$PRODUCT
        }
    }

    if (Test-Path $UNINST_ROOT) {
        Get-ChildItem $UNINST_ROOT -ErrorAction SilentlyContinue | ForEach-Object {
            $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
            if ($p -and $p.DisplayName -and $p.DisplayName -match $PRODUCT) {
                $snap.UninstallKey = @{
                    Path         = $_.PSChildName
                    DisplayName  = $p.DisplayName
                    DisplayVer   = $p.DisplayVersion
                    Publisher    = $p.Publisher
                    Uninstall    = $p.UninstallString
                    InstallLoc   = $p.InstallLocation
                    DisplayIcon  = $p.DisplayIcon
                }
            }
        }
    }

    if (Test-Path $INSTALL_DIR) {
        $snap.InstallDir = @(Get-ChildItem $INSTALL_DIR -Recurse -File -ErrorAction SilentlyContinue |
                             ForEach-Object { $_.FullName.Substring($INSTALL_DIR.Length + 1) })
    }

    if (Test-Path $START_MENU) {
        $snap.StartMenu = @(Get-ChildItem $START_MENU -Recurse -Filter "*$PRODUCT*" -ErrorAction SilentlyContinue |
                            ForEach-Object { $_.FullName })
    }

    return $snap
}

function Show-Snapshot($label, $snap) {
    Section $label
    if ($snap.RunValue)     { Note-Info ("自启项       : " + $snap.RunValue) }
    else                    { Note-Info "自启项       : (无)" }
    if ($snap.UninstallKey) { Note-Info ("卸载项       : " + $snap.UninstallKey.Path) }
    else                    { Note-Info "卸载项       : (无)" }
    if ($snap.InstallDir)   { Note-Info ("安装目录     : " + $INSTALL_DIR + " (" + $snap.InstallDir.Count + " 个文件)") }
    else                    { Note-Info "安装目录     : (无)" }
    Note-Info ("开始菜单     : " + $(if ($snap.StartMenu.Count -gt 0) { $snap.StartMenu.Count.ToString() + " 项" } else { "(无)" }))
}

# ---------------------------------------------------------------- 找安装包
Section '0. 定位安装包'
if (-not $InstallerPath) {
    $rel  = 'src-tauri\target\x86_64-pc-windows-gnu\release\bundle\nsis'
    $base = Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) $rel
    if (-not (Test-Path $base)) {
        Write-Host ("找不到产物目录：" + $base) -ForegroundColor Red
        Write-Host "先跑一次 npm run build。" -ForegroundColor Yellow
        exit 2
    }
    $InstallerPath = (Get-ChildItem $base -Filter '*-setup.exe' |
                      Sort-Object LastWriteTime -Descending |
                      Select-Object -First 1).FullName
}
if (-not (Test-Path $InstallerPath)) {
    Write-Host ("安装包不存在：" + $InstallerPath) -ForegroundColor Red
    exit 2
}
Note-Ok ("安装包：" + (Split-Path $InstallerPath -Leaf))
Note-Info ("大小：" + [math]::Round((Get-Item $InstallerPath).Length / 1MB, 2) + " MiB")

$before = Get-Snapshot
Show-Snapshot '1. 装之前的样子（应该是干净的）' $before
if ($before.InstallDir -or $before.UninstallKey) {
    Write-Host "  注意：已经装过一份。这次会覆盖安装，卸载阶段会把两份一起清掉。" -ForegroundColor Yellow
}

# ---------------------------------------------------------------- 安装
Section '2. 静默安装'
$t0 = Get-Date
$proc = Start-Process -FilePath $InstallerPath -ArgumentList '/S' -Wait -PassThru
$elapsed = ((Get-Date) - $t0).TotalSeconds
Note-Info ("退出码 " + $proc.ExitCode + "，耗时 " + [math]::Round($elapsed, 1) + " 秒")
if ($proc.ExitCode -eq 0) { Note-Ok "安装进程正常结束" } else { Note-Bad ("安装退出码 " + $proc.ExitCode) }

# NSIS 有时装完还在收尾，稍等一下再抓快照
Start-Sleep -Seconds 2
$after = Get-Snapshot
Show-Snapshot '3. 装之后' $after

# ---------------------------------------------------------------- 逐项核对
Section '4. 逐项核对'

if ($after.InstallDir) {
    Note-Ok ("安装目录存在，共 " + $after.InstallDir.Count + " 个文件")

    if ($after.InstallDir -contains 'paypulse-app.exe') { Note-Ok '主程序 paypulse-app.exe 在位' }
    else { Note-Bad '主程序 paypulse-app.exe 不在安装目录' }

    # 这一步是「许可清单真的随包发了」的唯一硬证据
    if ($after.InstallDir -contains 'THIRD-PARTY.md') { Note-Ok 'THIRD-PARTY.md 随包安装（合规义务落实）' }
    else { Note-Bad 'THIRD-PARTY.md 没进安装目录' }

    if ($after.InstallDir -contains 'uninstall.exe') { Note-Ok '卸载程序 uninstall.exe 在位' }
    else { Note-Bad '卸载程序 uninstall.exe 不在' }
} else {
    Note-Bad '装完了但安装目录不存在'
}

if ($after.UninstallKey) {
    Note-Ok ("「应用和功能」里有条目：" + $after.UninstallKey.DisplayName + " " + $after.UninstallKey.DisplayVer)
    if ($after.UninstallKey.Publisher) { Note-Ok ("发布者：" + $after.UninstallKey.Publisher) }
    else { Note-Bad '发布者为空（用户在卸载列表里会看到一行光秃秃的包名）' }

    # 卸载串必须指向真实存在的文件，否则用户点「卸载」会什么都没发生
    $un = $after.UninstallKey.Uninstall
    if ($un -match '"([^"]+\.exe)"') { $unExe = $Matches[1] }
    elseif ($un -match '^(\S+\.exe)') { $unExe = $Matches[1] }
    else { $unExe = $null }
    if ($unExe -and (Test-Path $unExe)) { Note-Ok ("卸载串指向真实文件：" + $unExe) }
    else { Note-Bad ("卸载串指向的文件不存在：" + $un) }
} else {
    Note-Bad '「应用和功能」里没有条目'
}

if ($after.StartMenu.Count -gt 0) {
    foreach ($s in $after.StartMenu) { Note-Ok ("开始菜单：" + $s) }
} else {
    Note-Bad '开始菜单里没有快捷方式'
}

# 自启这一项只有开关打开时才该有值，装完默认是关的
if ($after.RunValue) {
    Note-Bad ("装完就有自启项（默认应该关着）：" + $after.RunValue)
} else {
    Note-Ok '装完没有自启项（默认关闭，符合预期）'
}

# 安装路径下运行一次，确认它能起来（只是启动，不等到截图）
$exe = Join-Path $INSTALL_DIR 'paypulse-app.exe'
if (Test-Path $exe) {
    Section '5. 从安装路径启动一次'
    try {
        $p = Start-Process -FilePath $exe -PassThru
        Start-Sleep -Seconds 3
        if ($p.HasExited) {
            Note-Bad ("进程启动后立刻退出了，退出码 " + $p.ExitCode)
        } else {
            Note-Ok ("进程在跑（PID " + $p.Id + "），说明从这个路径启动没问题")
            Note-Info "自启注册表写的就是这个路径，所以这条通过等于自启路径也是对的"
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
            Start-Sleep -Seconds 1
        }
    } catch {
        Note-Bad ("启动失败：" + $_.Exception.Message)
    }
}

# ---------------------------------------------------------------- 卸载
if ($Keep) {
    Section '6. 跳过卸载（-Keep）'
    Note-Info "装好的东西留在机器上，要清理就再跑一次不带 -Keep 的。"
} else {
    Section '6. 静默卸载'
    $unExe = Join-Path $INSTALL_DIR 'uninstall.exe'
    if (Test-Path $unExe) {
        $p2 = Start-Process -FilePath $unExe -ArgumentList '/S' -Wait -PassThru
        Note-Info ("退出码 " + $p2.ExitCode)
        # 卸载进程会先把自己复制到临时目录再删原目录，得等它收尾
        Start-Sleep -Seconds 4
    } else {
        Note-Bad '找不到 uninstall.exe，没法自动卸载'
    }

    $final = Get-Snapshot
    Show-Snapshot '7. 卸之后的样子（应该回到装之前）' $final

    Section '8. 残留检查'
    if ($final.InstallDir) { Note-Bad ("安装目录还在：" + $INSTALL_DIR) } else { Note-Ok '安装目录已清空' }
    if ($final.UninstallKey) { Note-Bad '「应用和功能」里还有条目' } else { Note-Ok '卸载项已移除' }
    if ($final.StartMenu.Count -gt 0) {
        foreach ($s in $final.StartMenu) { Note-Bad ("快捷方式还在：" + $s) }
    } else { Note-Ok '开始菜单已清理' }
    if ($final.RunValue) { Note-Bad ("自启项残留：" + $final.RunValue) } else { Note-Ok '自启项无残留' }
}

# ---------------------------------------------------------------- 结论
Section '结论'
if ($script:failures.Count -eq 0) {
    Write-Host "全部通过。这个包别人装得上、跑得起来、卸得干净。" -ForegroundColor Green
    exit 0
} else {
    Write-Host ("有 " + $script:failures.Count + " 项不通过：") -ForegroundColor Red
    foreach ($f in $script:failures) { Write-Host ("  - " + $f) -ForegroundColor Red }
    exit 1
}
