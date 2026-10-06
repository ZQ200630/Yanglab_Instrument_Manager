# Yang LAB INSTRUMENT CONSOLE

## Current real-only local Host architecture (2026-10-06)

This is the first, local-only stage. Each Windows computer runs one independent
Host and one VISA Python worker. Multiple desktop GUIs can observe the same Host;
only one client can control a given device or Fiber setup. Remote hosts,
Tailscale pairing and network control are not implemented in this stage.

The production GUI uses authenticated, current-user Windows named pipes and the
strict v3 worker protocol. It does not expose raw serial, VISA or generic worker
commands. The older v2 GUI is retained only as a regression fixture.

Instrument pages and Overview cards use compact, static 48 px perspective icons
beside the instrument name. The production GUI no longer mounts the large 3D
preview, camera controls or WebGL viewer. Icons identify a device family only;
they do not represent current readings, output state or stage position. Custom
device icons remain generic until their physical appearance is verified.

### Configure and operate

1. Launch the app to automatically attach a compatible real-only local Host. If it is absent, the app
   starts it once using the configured Anaconda **VISA** `python.exe`. Use **Settings** for
   **General**, **Local Host** (Python, name and safe lifecycle actions),
   and **Remote Hosts** (explicitly unavailable in this stage). Startup choices are
   saved locally and take effect after a safe Host restart. There is no instrument backend selector.
   An obsolete Host is rejected rather than silently reused. No instrument session,
   control lease or output is restored. Startup opens no instruments unless a
   separate, previously authorized safe check policy permits a temporary probe.
   Unknown readiness, untrusted peers and retained ownership never cause an
   automatic retry. Overview has no connection, settings or **Add New** action.
   The tray icon keeps the Host visible after GUI windows close;
   **Open Console** reopens management and **Stop Host safely** requires confirmation.
2. In **Device setup**, select **Add New**, category, supported model and connection
   profile. Configure the address, then select **Test Connection**. Only a current,
   real identity-bound verification proof permits **Add and Save**. Unsupported models are
   marked **Driver required**. Legacy non-real registrations become editable imported drafts
   after an immutable configuration backup; their old proof never authorizes real hardware.
3. Voltage and Gain require a separately confirmed **Prepare supervised session**;
   testing never silently opens these devices. Source opening may zero all channels;
   Gain opening may invoke safety shutdown. Other profiles disclose port-open effects.
4. Open the exact instrument instance and select **Connect**. After connection
   consent, the GUI automatically obtains exclusive authority and checks the
   current Host/context before opening. **Disconnect** safely closes and releases
   it; the GUI waits for confirmed cleanup before offering Connect again. Another
   controller is shown as **In use · Read only**, never silently displaced.
   Sensitive output actions and Fiber baseline/nominal authorization retain their
   separate confirmations. Unknown outcomes offer **Check status** and safe
   disconnect, never automatic replay. Internal IDs and receipts are in collapsed
   **Diagnostics**, not success notifications or extra permission buttons.
   A safely paused instrument exposes **Resume controls** only to its current
   controller; it never requires reconnecting just to resume. Disconnect requires
   an accepted cleanup request and a fresh, post-acceptance Host snapshot before
   claiming release. A lost authority reply cannot replay Connect or stop another
   controller; own-session recovery remains bound to authenticated boot/epoch.
   A safely paused instrument exposes **Resume controls** only to its current
   controller; it never requires reconnecting just to resume. Disconnect requires
   an accepted cleanup request and a fresh, post-acceptance Host snapshot before
   claiming release. A lost authority reply cannot replay Connect or stop another
   controller; own-session recovery remains bound to authenticated boot/epoch.
5. Register MDT controllers individually, then create a one- or two-member **Fiber
   setup**. Known serial identities determine sides, never COM order. Operate stages
   only through the setup. Safely release a setup before removing it; removal holds
   piezo voltage and does not zero, roll back or adopt a baseline.

Configuration is locally persistent, atomically replaced and revision-checked.
Startup preferences are separately stored in the native app-config directory as
bounded, versioned `preferences.json`; corrupt or unsupported files block automatic
startup rather than being overwritten. Device registry and Host settings retain
their existing Host-owned storage. **Device setup** is the only **Add New** entry.
Automatic checks default to off. Enumeration and active read-only checks require
separate explicit policies; only reviewed safe profiles may auto-open. Busy or
controlled devices use cached evidence, not competing probe sessions. Old unstamped
records remain metadata-only and require real identity re-verification.

**LOCAL / REMOTE** describe source. **ONLINE / UNKNOWN** describe recent communication,
not physical output or motion. **Host offline**, **Session closed**, control ownership,
cleanup **RETAINED**, and sample age are separate facts. A failed or stale query does
not prove that outputs are off. Result files and operation receipts are evidence of
software outcomes, not independent physical measurements.

Closing an observer GUI does not stop the Host or another controller. Closing a
controller GUI first cleans only its own domains; failed cleanup retains the window
and management entry. Stopping the entire Host is a separate confirmed action.
Voltage shutdown attempts immediate zero, Gain disables current before TEC, and
Fiber/MDT faults stop and hold. A non-returning old call retains responsibility.

Safety limits remain enforced by `Code/Utils` and `Code/Setups`: Source 0–14 V,
steps at most 0.1 V/50 ms; Gain 0–200 mA and 15–40 °C with the five-second TEC
stability interlock; piezo at most 75 V, 0.1 V/50 ms steps, no automatic zero;
toward-chip Fiber moves at most 0.2 um and other per-axis moves at most 1.0 um.
OSA and PM400 preserve front-panel settings as specified by their drivers. Stage
displacement is an explicitly authorized, session-only open-loop estimate, not a
measured position. Device models are identification aids, not collision checks.

Current-source installation and full real-device acceptance remain unfinished.
OSA identity-only evidence exists; it does not prove trace acquisition or physical outputs.
Follow `AGENTS.md`: separately authorize
enumeration, read-only connection, then a reversible action. Use the diagnostics
in `Code/Debugs`, with every Python command in the VISA environment. Hardware-free
regressions do not grant any of these real-hardware authorizations.

### Offline build

The OSA data page uses **Read trace** as its main action. Sweep initiation is
separate and checks the instrument's current SINGLE/AUTO mode. Native W/dBm
samples are archived by the owning Host, verified before display, and exported
with the original manifest through the native folder dialog. History access is
data-only and does not acquire control. See the 2026-10-06 checkpoint in
`docs/acceptance.md` for source/build evidence and the still-pending installer,
physical archive and native-window checks; this is not an installed release.

Use the already configured MSVC/Rust cache described below, then run full Python,
Node and Rust regressions. Run `App/scripts/build-host.ps1 -Offline` **before**
`cargo tauri build --ci -- --locked`. The helper stages the fixed Host sidecar;
missing packaged Host or worker resources fail closed with no release source
fallback. No Python interpreter is bundled; install/configure VISA separately.
The Host binary is feature-gated (`host-bin`) and shipped only as the explicitly
staged sidecar, avoiding a second Cargo binary overwriting it in the installer.
The package includes only `Code/Utils`, `Code/Setups`, `Config`, `App/worker`,
`App/catalog` and required package initializers. Diagnostic/test/experiment code,
retired emulator sources and GPU preview libraries are not bundled.

Run safety checks in VISA before hardware:

```powershell
& 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe' -B -m unittest discover -s Code/Debugs -p 'test_*.py' -q
& 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe' -B -m unittest discover -s App/tests -p 'test_*.py' -q
$nodeTests = Get-ChildItem -LiteralPath App/tests -Filter '*.test.mjs' | Select-Object -ExpandProperty FullName
& 'C:/Users/PIC_YangLab/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node.exe' --test $nodeTests
```

Native Host process contracts inject finite transport scripts and require an idle
local Host namespace; they must not displace a running instrument owner. Browser
preview is a visual test only and cannot operate instruments. Only small static
identification icons remain; no interactive 3D preview is shipped.

## Historical v2 notes — not the current production architecture

The following retained notes describe earlier candidates and their evidence only.
Do not use their role-based ownership, preview or build claims as current v3
acceptance. Their obsolete simulation and 3D instructions are not supported by
the current executable. Current verification is recorded with the implementation ledger.

2026-10-05 UI update: all application labels, dialogs and PM400 catalog labels use English. The Overview page no longer displays the 3D workbench; individual instrument pages retain their device models. The application title and sidebar brand are Yang LAB INSTRUMENT CONSOLE. Instrument protocols and safety limits are unchanged.

2026-10-05 源码状态：本轮仅修复“显式恢复成功后，较早发出的状态回复重新锁存 UNKNOWN”的竞态。恢复后暂不放行普通控制，忽略该角色恢复前查询中的旧权限/身份数据，并等待一次有界、共享的恢复后状态确认；新的限制、失败和全局关闭仍会封锁。离线 Node 115/115、App Python 193/193 通过，独立限域复核待协调者执行。本轮没有重建或修改任何产物：下述 `62949f9` 候选仍含此缺陷，目录中的 `REVIEW_STATUS.md` 仍为未批准交付；旧构建记录不代表本轮源码已打包。原生 UI、安装器内容和实机验收仍未完成。

Windows Tauri 2 上位机，复用仓库中的 Python 驱动。截至 2026-10-04 的 v2 源码曾通过离线软件回归并完成 Windows 编译、NSIS 打包及隔离目录资源验收；该旧构建尚未安装或完成原生窗口操作验收，且后续复核发现了上面注明的恢复竞态。此前版本的独立目录安装和原生交互记录只适用于各自注明的旧产物。真实硬件仍未验收，不能把旧构建当作已投入实验的控制台。分层证据见 `docs/acceptance.md`。

2026-10-04 最终复核修正：当前候选目录为 `Result/console/safety-dispatch-62949f9c-20261004`，二进制与资源的实际源码提交为 `62949f9ca0f009db5ff89bf2cc509d0a4afa5006`；后续 `5d7c5f0` 只修正离线测试同步，之后的文档提交也不改变二进制来源。旧 `safety-dispatch-84aa3fda-20261004` 保留为修正前历史候选，不能作为本次修复产物。新候选的文件哈希、101 个资源逐项比对、离线测试与原生未验收清单见验收记录顶部。

2026-10-03 原生增量：安装版总览/双侧位移台三维渲染、生产确认框、模拟清单与 Fiber 连接、左侧双重基线授权、+0.100 µm 动作的取消/接受分支、1000× 显示，以及模拟会话窗口关闭已直接观察通过。缺资源副本明确拒绝启动、不回退源码；本轮发现并修复空闲窗口缺少 Tauri 关闭权限的问题，重新打包后初始空闲和启动失败窗口均可退出。其余设备的完整原生动作流程、失败清理窗口路径和真实硬件仍未验收。模拟通过不授权真实连接，也不等同于物理验收。

## 边界与设备

| 面板 | 控制来源 | 启动/连接行为 |
| --- | --- | --- |
| OSA AQ6370 | `Code/Utils/osa.py` | 连接只读；采集沿用前面板设置 |
| 八通道电压源 | `Code/Utils/voltage.py` | CH340；连接会令全部通道归零，限 0–14 V |
| Gain Chip Driver | `Code/Utils/gain.py` | CP210x；限 0–200 mA、15–40 °C；先 TEC 稳定后电流 |
| PM400 | `Code/Utils/pm400.py` | 连接/正常关闭保留设置；有限、带能力门控的功能目录 |
| 左/右光纤台 | `Code/Setups/fiber_coupling.py` | 两台 MDT693B 按 USB 序列号固定侧别；连接只读，故障停住并保持电压 |

界面没有串口或 VISA 直通命令。Tauri Rust 宿主持有一个长期运行的 Anaconda `VISA` Python worker；该进程持有所有驱动和进程内的资源预约。一个 OSA 和一个 PM400 可以使用不同的 VISA 资源，但不能绑定同一个规范资源名。其他应用仍可能占用端口，不受本进程预约保护。

当前前端、Rust、Python worker 和模拟预览统一使用一行一请求、按 ID 关联回复的 v2 协议；不回退到 v1。普通操作按 OSA、Voltage、Gain、PM400、Fiber 五个 role 调度，Fiber 左右两侧仍共享同一 setup 调度域。一个 role 忙碌时，其它 role 的安全入口与缓存状态读取仍可服务。普通队列有界；同一 role 的旧连接身份、旧代次和已排队输出不能在重连或停止后获得新的操作权限。请求超时或传输失联表示结果未知，不等于取消或驱动已停止。

Rust 命令先取得有限名额再把阻塞等待移出异步执行线程；等待责任上限为 31 个普通请求、1 个查询、8 个按角色安全请求及 1 个关闭请求，启动与关闭另有单次责任门控。单执行线程的离线管道测试确认：OSA 回复被阻塞时，归零、状态、设备断开与会话关闭仍能进入；会话关闭等待中仍能读缓存状态。前端所有 `status` 调用合并，`ping` 与 `status` 共用一个实际查询槽，不积累轮询队列。已知未发送的准入拒绝仍是失败，但不冒充设备结果未知；真正超时、传输失联、旧身份回复仍保守封锁。

Voltage 归零、Gain 关电流/TEC 和设备断开是受控安全意图。收到停止意图后，worker 先封锁该 role 的普通写入并作废未开始的旧请求，再优先尝试相应驱动公共 API；若先前已有调用进入驱动，在它退出后还要完成最终安全调用，才可报告该请求最终完成。等待旧调用期间仍显示处理中，不能把首次安全调用返回当成最终关断。成功的保留连接安全操作进入 `STOP_HELD`，须在当前连接身份下显式恢复普通控制；恢复本身不写输出、不重放旧输入，也不重建 Fiber 位移权限。全局关闭并行安排各 role 的清理责任，失败或未确认释放保留独立尝试报告。软件优先级不保证硬实时响应，驱动回复、资源释放、主机侧零证据和物理测量是不同事实。

失败关断或超时后，成功的最终安全重试与当前代次 `STOP_HELD` 证据允许操作者显式申请恢复；普通健康状态和迟到回复都不会自行清除未知锁存。恢复时仍由后端检查健康读回和无活动调用，宿主检查超时请求终态与限制代次；观察恰好进行中也会明确拒绝，须待其完成后再显式操作。只有当前身份下获准的恢复回复可解除界面限制，绝不恢复旧 Fiber 基线或自动执行草稿。

Gain 面板将温度、目标、TEC 状态、电流设定和电流开关分别读回、计龄；未成功读过为 `unknown`，超过 5 s 为 `stale`，旧值不显示成确认 Off。状态刷新约每 2.5 s 安排一次，积压合并；五个字段不是同一时刻的原子测量。开启电流前要求本连接关键字段新鲜、TEC 已确认开启，最终五秒稳定联锁仍由驱动判定。按钮固定为显式“开启”“关闭”，稳定等待不会自动开电流。`read_tec_enabled()` 和 `read_current_enabled()` 可能触发驱动的安全联锁关断，因此此类读取并非绝对无输出副作用。

打开程序不会自动连接仪器。页面重载时只读探测并重新附着同一宿主中仍存活的 worker，不重复启动或打开端口。枚举、连接和任何可逆输出操作各自独立确认。OSA 的连接不修改前面板设置，但后续关闭可能中止扫描。电压源的连接**不是只读**：它会归零所有通道。Gain 连接若发现“电流开而 TEC 关”，驱动可能执行安全关断。关闭时电压源尝试归零、Gain 先关电流再关 TEC，光纤台保持压电电压。窗口关闭事件在 worker 存活时先请求有序清理；无报告、失败或超时时保持窗口打开以供核对。原生窗口已验证取消关闭、未连接模拟会话和已连接模拟 Fiber 会话的正常关闭；2026-10-03 另用不含仪器驱动的管道夹具验证：收到失败清理报告后子进程退出 19，窗口保持打开并分别显示异常退出和原始报告，之后再次显式关闭可退出。这不证明真实驱动失败或仍存活的 worker 的原生 UI 行为。界面上的“命令成功”与“资源释放”都不是物理零电压证明；总览会分别显示清理步骤、未释放资源和主机遥测零证据。

OSA 光谱和游标只属于当前连接会话；成功断开或重新连接时会清空，避免把旧设备的曲线显示在新设备身份下或继续导出。

配置页区分待使用的 Python 路径与工作进程报告的实际 Python/代码目录。启动成功后显示运行路径；页面重载附着时通过只读 `ping` 重新取得，并以实际解释器覆盖输入框中可能过时的存档路径（不会自动保存设置）。关闭后撤销运行身份，输入框仍保留该路径供下次手动启动。若重读失败，运行路径不显示，但保留已有 worker 和有序关闭入口。2026-10-03 原生安装版启动已观察到实际 VISA 路径及安装目录代码根；重载附着与异常回读仍应区别于该次正常启动证据。

Rust 宿主在安装模式只从打包资源目录启动 worker；资源缺失时拒绝启动，不回退编译机的源码目录。源码回退仅限 Tauri 开发模式且调试构建；直接 `cargo build --release` 生成的程序也不允许回退。该路径选择已有 Rust/WASI 和 Windows 原生单元测试。NSIS 构建、独立目录安装及窗口内启动模拟 worker 已通过；2026-10-03 对同哈希安装 EXE 的缺资源副本实测得到 `instrument worker package is missing from bundled resources`，没有 Python worker，并可关闭窗口。

工作进程异常退出后，显式「关闭工作进程」只有在操作系统确认该进程已退出时才释放宿主所有权，允许操作者重新启动一个未连接的新会话；不会自动重启。报告仍明确标记清理失败、资源释放未确认和物理输出未知。若退出前曾收到清理报告，总览单独保留其原始内容；进程退出事实不能把该报告改写成成功。仍存活的进程在关闭失败时继续保留，不能被新会话替代。超时后的失败状态回读也不会重新放行修改命令。

单设备断开后，worker 只有在驱动报告 `DISCONNECTED` 且公开的资源释放/端口状态没有矛盾时，才解除该角色的资源预约。若证据缺失、读取失败或显示端口仍打开，角色会保留供重试清理；总览标为 `RETAINED`，不把它说成已离线。电压源的上次命令目标此时变为未知。这些都是主机侧判断，不能替代仪器输出实测。

光纤台使用实验室坐标 `+X` 向右、`+Y` 向实验台后方、`+Z` 向上。左侧靠近芯片是 `+X`，右侧是 `−X`。3D 示意可用鼠标拖动或在聚焦后用方向键旋转，`Home` 恢复默认视角；这些按键只改变视角，不下发位移。界面显示的位移必须先由操作者采用当前读数为基线，再单独确认授权 MAX312D 名义换算；它只是会话内开环估计，不是位移测量。外部手动移动后需重新采用基线。每次靠近芯片限 0.2 µm，其余每轴限 1.0 µm。压电输出上限 75 V，故障不自动归零。

键盘聚焦的导航项、3D 示意和设备按钮在定时状态刷新后会恢复到同一可用控件；若控件消失或变为禁用，则不自动转移焦点。Gain 的 TEC/电流开启与关闭是固定含义的独立按钮。恢复焦点本身不会执行仪器动作。定时状态刷新会保留同一已确认设备上尚未提交的表单输入，并重新计算 Fiber 位移预览；若资源身份缺失、连接状态异常或角色切换，则不继承旧输入。输入期间到达的状态回复不会打断当前编辑，设备动作仍需单独点击并确认。配置页的资源下拉菜单在运行模式、资源所有权和连接健康状态未变化时同样保留实际控件，避免正常轮询关闭正在选择的原生菜单；状态失联或所有权变化仍立即重绘安全提示和禁用状态。

PM400 的探头能力会在界面中决定操作是否可用；连接或状态不可靠时测量、设置和高级命令按钮禁用并显示原因。归零/校准、探头响应系数、适配器类型、复位等需再次确认，且仍由底层驱动复核。高级命令旁显示本次连接中上次成功返回的内容，例如错误队列；断开或重连后不沿用旧结果。「系统/状态」页另显示驱动缓存的上次受检操作前已存在错误，不额外查询仪器，也不把缓存当作当前错误队列。系统没有提供原始 SCPI 输入框。所有 Python 和前端离线测试在执行真实硬件动作前运行。

## 离线验证

在仓库根目录运行：

```powershell
& 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe' -B -m unittest discover -s App/tests -p 'test_*.py' -q
& 'C:/Users/PIC_YangLab/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node.exe' --test App/tests/*.test.mjs
```

若本机已有 Rust 的 `wasm32-wasip1` 标准库目标，可在缺少 Windows SDK 时独立运行路径选择模块的 Rust 单元测试：先用 `rustc --test --edition 2021 --target wasm32-wasip1 App/src-tauri/src/worker_root.rs -o <临时目录>/worker_root.wasm` 编译，再用 `node --experimental-wasi-unstable-preview1 App/tests/run_rust_wasi.mjs <临时目录>/worker_root.wasm --test-threads=1` 执行。这不验证 Tauri 宿主接线或原生安装包。

前端纯静态视觉样例在 `App/tests/visual-preview.html`。它显示虚构读数，不连接 worker，也没有真实动作能力。可以从仓库根目录启动只绑定本机的静态服务，再浏览 `http://127.0.0.1:8766/tests/visual-preview.html`：

```powershell
conda run -n VISA --no-capture-output python -B -m http.server 8766 --bind 127.0.0.1 --directory App
```

若要测试真实前端与 Python 模拟控制器之间的完整按钮流程，使用**仅供测试**的动态预览，在仓库根目录运行以下命令后浏览 `http://127.0.0.1:8766/`（不要同时运行上面的静态服务）：

```powershell
conda run -n VISA --no-capture-output python -B -m App.tests.preview_server --port 8766
```

该预览只绑定 `127.0.0.1`，拒绝真实模式，不实例化硬件驱动；其测试注入脚本会**自动接受确认框**以便浏览器联调。模拟清单明确列出虚拟 VISA/串口资源及固定左右光纤台，不能当作本机实物枚举结果。生产 Tauri 包不包含这段脚本，仍逐项要求人工确认。动态预览不能替代原生窗口或实机验收。OSA 曲线点击游标读取原始点；电压和温度趋势使用固定物理量程、最近主机样本，绝非独立物理测量。

模拟 worker 必须显式使用 `--simulate`。该模式不实例化真实硬件驱动；`--real` 只是允许后续经单独确认的真实连接/动作，启动本身不打开仪器。

## Windows 构建

需要 [Tauri 2 Windows 前置条件](https://v2.tauri.app/start/prerequisites/)：Rust MSVC 工具链、Microsoft C++ Build Tools/Windows SDK、WebView2，以及本机 Anaconda `VISA` 环境。2026-10-02 已安装 Build Tools 2022（MSVC 14.44.35207）及 Windows SDK 10.0.26100.0，系统已有 WebView2 154.0.4258.48；本机已成功完成原生测试、release 编译和 NSIS 打包。安装成功、离线测试通过与真实仪器验收是不同结论。

当前构建复用本机已经缓存的工具链与锁定依赖；不执行 `cargo install`、下载或升级 lockfile。先在仓库根目录运行上面的 Python/Node 离线测试，再在 `App/src-tauri` 执行：

```powershell
cargo test --offline --locked
cargo fmt --check
cargo tauri build --ci -- --locked
```

本机工具链位于 D 盘项目临时目录，不写入永久 `PATH`。以上 Rust 命令前，在同一个新的 PowerShell 会话中先运行：

```powershell
& 'D:/SoftwareInstaller/MicrosoftBuildTools2022/Common7/Tools/Launch-VsDevShell.ps1' -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
$buildRoot = 'D:/Qian/Codex_Project/SIL_Experiments/tmp/tauri-build'
$env:RUSTUP_HOME = "$buildRoot/rustup"
$env:CARGO_HOME = "$buildRoot/cargo"
$env:CARGO_TARGET_DIR = "$buildRoot/target"
$env:TEMP = "$buildRoot/temp"
$env:TMP = $env:TEMP
$env:CARGO_NET_OFFLINE = 'true'
$env:PATH = "$buildRoot/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin;$env:CARGO_HOME/bin;$env:PATH"
```

原生管道测试使用本机 `D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe`。如需验证实际安装资源，可在 `cargo test` 前设置 `SIL_TEST_INSTALLED_ROOT` 为安装目录；它只改变测试的代码来源，仍强制 `--simulate`，不执行真实枚举或连接。

安装包内打包 `Code/`、`Config/`、`App/worker/`；Python 解释器不随包打入。首次启动应选择本机 `VISA` 环境的 `python.exe`，再先用模拟模式验收。选择真实模式之前仍要依 `AGENTS.md` 分别授权枚举、只读连接、可逆动作，实际电压/电流/位移状态要在仪器端独立核验。

## 三维设备视图（2026-10-02）

各仪器页面与总览现在提供本地 Three.js 网格模型：NanoMax 双侧台子、MDT693B、AQ6370 系列、PM400、电压源及 Gain Driver。无需联网，不需要启动 worker 即可查看。拖动、方向键、滚轮与正负号只控制相机；Home 或“复位视角”恢复观察方向，不执行仪器动作。

位移提供 1× 与 1000× 显示；倍率不进入硬件命令。青色线框为尚未执行的输入预览；实心台面位置是已有基线下的会话内开环估计，不是编码器实测。未知/故障状态只显示结构参考姿态，不代表电压归零。芯片和手动微分头不随压电命令移动。输入框聚焦时仍读取状态并更新三维真实性，不覆盖未提交输入。

所有模型都是原创近似网格，不是厂商 CAD。自制设备、夹具与整体布局明确标为暂定，需要实物照片/尺寸后校正；OSA 外形参考 AQ6370E，不证明当前仪器的型号后缀。PM400 探头型号未确认，不建造虚假的探头模型。三维画面不用于碰撞或实际间隙判断。

GPU 不可用或上下文丢失时显示错误，仪器控制面板仍可用；“复位视角”可显式重建失败的三维视图。运行库版本、来源、许可和校验值见 `web/vendor/three/PROVENANCE.md`。厂商 CAD 的打包分发许可尚未确认，未随程序分发。

## 配置与故障

**当前验收限制（2026-10-04）：**旧版全局 busy/串行 worker 与 Gain 整组样本时间问题已在当前 v2 源码中重构，并通过离线回归；本次打包产物的原生窗口安全操作、迟到结果与失败关闭流程仍未验收，真实仪器响应时间和物理输出也未测量。优先调度不是独立急停通道。上述 2026-10-03 缺陷记录仍是旧版证据，不能套用到当前产物，也不能用离线通过代替实机验收。

界面的资源选择存于 `%LOCALAPPDATA%/SILInstrumentConsole/settings.json`（仅路径与角色绑定，不存储已授权/已使能状态）。连接失败或 worker 响应丢失时，界面应显示不确定结果；不要因为按钮恢复或进程退出就推断输出已经关闭。驱动调用一旦开始，即使它随后抛出 `ValueError`，也不能据此推断动作未发生；提示结果未知时先读回实机状态，不要盲目重试。若提示驱动命令已返回、但结果编码或随后状态回读失败，同样不能把它当作“动作未发生”；连接资源仍由 worker 保留，需先核对仪器状态并有序清理。先从仪器面板独立检查，保存清理报告，再决定是否重试。真实硬件验收记录应放在 `docs/acceptance.md` 与 `Code/Debugs` 诊断输出中。
