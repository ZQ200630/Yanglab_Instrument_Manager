# Local Host and Device Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建立安全优先的独立本机 Host，支持动态登记多台同型号仪器、按实例控制，并交付白色英文原生界面。

**Architecture:** 从现有 Rust worker 管理器提取共享运行时，由独立 Host 持有唯一 Python worker。GUI 经受 Windows 用户访问限制的本机 IPC 访问 Host；worker v3 将固定 role 改为登记执行域，保持原有调度、安全屏障和清理证据。设备注册、在线检查和控制权是三个不同流程。

**Tech Stack:** Windows、Tauri 2、Rust/serde、Windows named pipes、原生 JavaScript、Anaconda VISA Python、unittest、Node test runner；复用现有仪器驱动。

**Spec:** [已批准设计](D:/Qian/Codex_Project/SIL_Experiments/docs/superpowers/specs/2026-10-05-lab-host-design.md)。执行者须先完整阅读该设计及目标 checkout 的 AGENTS.md。

## Global Constraints

- 第一阶段只交付本机 Host、IPC、动态设备及 setup 管理、实例调度、原生浅色 UI。HTTPS/WSS、配对、远程共享和 Tailscale 部署属于第二阶段；第一阶段不生成假 REMOTE 设备或可用的远程连接按钮。
- 应用名称为 `Yang LAB INSTRUMENT CONSOLE`；所有软件标签、消息、确认、错误和按钮使用英文。设计和实施说明允许中文。
- 所有 Python 命令使用 `D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe`。本阶段全部验证为离线 fixture 或明确 `--simulate`；不枚举、不连接、不操作真实硬件。
- 驱动仍在 `Code/Utils`，Fiber 运动仅经 `Code/Setups/fiber_coupling.py`；仪器端口不能在 App 层直接打开。`reference_code`、实验代码及现有实验配置不改。
- Voltage 0–14 V，正常步进 ≤0.1 V、间隔 ≥50 ms，失败/关闭立即尝试八通道零；Gain 0–200 mA、15–40 degC，TEC 开启且目标 ±0.2 degC 连续五秒稳定后才可开启电流，关闭顺序为电流、TEC。
- OSA 保留前面板测量设置；PM400 保留设置并按 sensor capability 门控，危险维护操作逐次明确确认；不同规范 VISA 资源独立，同一资源不重复持有。
- MDT693B 75 V，正常步进 ≤0.1 V、间隔 ≥0.050 s；故障 stop-and-hold，不自动零压。连接和恢复不建立位移权限。
- Fiber 左序列号 `2110148249-10`、右 `160721175410`；+X 向右、+Y 远离操作者、+Z 向上，左 XYZ→MDT YXZ、右→XYZ。朝芯片左 +X/右 −X 每次 ≤0.2 um，其它每轴 ≤1.0 um；MAX312D 名义换算与基线采纳分别明确授权，估计仅属于当前会话。
- 清理报告不可变；生命周期调用成功、资源释放、Host 读回零和物理测量是不同证据。借用 VISA manager 必须活到所有依赖驱动确认释放。资源不明时保留责任对象，不能删除记录后假称已释放。
- 本机注册表 `version=2`；worker 启动固定选择 v2 或 v3，严格拒绝混版及静默降级。保留五种原始结果阶段及现有恢复竞态回归。
- 登记上限 64 台实体设备、16 个 setup；含验证草稿的活跃执行域最多 64 个。普通责任每域 1 项、全 Host 31 项；安全意图每域最多 3 项，连同 1 查询和 1 shutdown 总责任 225 项，回复最多 226 项含 1 协议错误帧。
- 每 Host 最多 16 个 GUI 会话；状态查询合并为 1 项、约 2.5 s；控制心跳 2 s、租约 10 s、准入复核单调期限。每 GUI 待发 256 项且 ≤1 MiB；遥测合并，责任事件不丢弃，慢消费者断开并清理其控制域。
- IPC 普通控制帧 ≤64 KiB，先限长再解析；结果通过独立分块读取，单次结果 ≤16 MiB。终态索引最多 4096 条、正常保留 24 h；未终态不淘汰，退役消息序号不重放。
- worker action 180 s、connect/disconnect 90 s、status 5 s、其它 RPC 30 s；Host 准入等待 5 s 不表示取消已开始的驱动调用。确认凭据与登记证据各 60 s。
- 默认不主动开端口。自动枚举和只读探测分别取得设备专属许可；空闲主动检查默认 30 s、最短 10 s、失败退避 30/60/120/300 s；每设备 1 项、整个 Host 同时 1 项。既有 Gain watchdog 不改变。
- 不安装开机服务、不自动恢复输出、不强杀 Host/worker、不修改 AWS 或私人 Tailscale、不提交或合入 main。每个任务的“检查点”替代技能模板的自动 commit；只有新授权才允许提交。

## Review Focus

1. VISA 别名、Serial/ASRL 或 setup 子设备指向同一物理资源：重复打开必须在驱动调用前拒绝；未知独立性也拒绝。归属 Task 5。
2. OSA 普通 close 中止扫描、Voltage/Gain connect 有输出副作用，以及串口 DTR/RTS 风险：只读验证不得偷偷走这些生命周期。归属 Task 6。
3. 磁盘满或原子替换失败：不发布半条登记，不丢掉受监管会话；普通操作未落盘不执行，安全关闭仍可准入。归属 Tasks 2、7、12。
4. 两 GUI 竞态、休眠恢复、旧连接迟到结果：只清理失权者的域，不能恢复旧控制权、Gain 证据或 Fiber 基线。归属 Tasks 4、11、13、15。
5. 第二 Host、残存 worker、PID 复用和安装缺少 sidecar：启动失败封锁，不杀进程、不绕到另一套 worker，也不以源码回退掩盖缺包。归属 Tasks 10、16。

---

## 执行位置与检查点

实际 App checkout 为 `C:/Users/PIC_YangLab/.codex/worktrees/tauri-console/SIL_Experiments`，当前 detached HEAD `f81e674`。以下 `App/...` 和 `Code/...` 均相对此 checkout；计划文件保存于主工作目录的 `docs/superpowers/plans`。主目录目前没有 App，不能把源码复制过去或假设已合入。

执行前依 using-git-worktrees 技能检查并复用已附加 checkout，读取全部适用 AGENTS.md。记录 HEAD、git status、23 个既有 App 修改文件的内容摘要和主目录 7 个既有实验修改；不得 reset/覆盖。若该 checkout 不在可写范围，先取得对应写入权限，不创建影子副本规避边界。

在改动前运行文末命令 V1–V3，以及 V4 中现有 cargo test/fmt 两项，保存基线；此时不运行尚未创建的 build-host.ps1。失败先定位并区分原有问题，不将它们冒充新实现已通过。每任务独立完成红→绿测试，记录修改文件与证据并停在检查点；不自动 git add/commit。所有 fixture 构造器只能创建模拟驱动；调用真实驱动工厂即令测试失败。

## 文件职责

| 文件组 | 职责 |
| --- | --- |
| `App/catalog/devices.json` | 稳定型号、连接 profile、依赖、公开操作、探测及生命周期副作用目录 |
| `App/worker/catalog.py`, `contracts_v3.py`, `domains.py` | 目录校验、严格 v3 契约、按实例证据与适配器；v2 保持兼容 |
| `App/worker/resources.py`, `verification.py`, `availability.py` | 资源互斥、验证责任、设备检查；不直接实现硬件协议 |
| `Code/Utils/common.py`, `osa.py`, `pm400.py`, `mdt693b.py` | 新增公开、可追责的只读身份探测；原正常生命周期不改变 |
| `App/src-tauri/src/lib.rs`, `runtime.rs`, `worker.rs` | 两个进程共用运行时，旧 Tauri worker 包装器逐步退出生产入口 |
| `App/src-tauri/src/host/` | contracts、catalog、registry、verification、availability、instance、ipc、leases、operations、events、results、service、tray，各一项职责 |
| `App/src-tauri/src/bin/yang-lab-host.rs`, `host_client.rs` | 独立 Host 入口和 GUI 本机客户端；GUI 不拥有 worker |
| `App/web/host-client.js`, `device-store.js`, `routes.js` | Host 会话、按完整实例键保存状态、无硬件副作用的导航 |
| `App/web/setup.js`, `overview.js`, `panels.js` | Add New 流程、实体设备概览和现有仪器面板的实例化 |
| `App/tests/`, `Code/Debugs/test_*.py` | 离线回归、协议/多 GUI/原生模拟验收；真实诊断仍单独授权 |

不重写整个 controller 或 scheduler：提取共同生命周期、巡检和安全核心，保留 v2 外观。只在明显违反单一职责处拆文件；不引入另一个前端框架或第二套驱动。

## 公共契约

这些名称贯穿任务。所有 ID 是 Windows 安全随机生成的 128 位小写 hex 标识，不用显示名、COM 号或类别作 ID；epoch 和序号为 0..9007199254740991 的整数。

- `DomainRef = {kind: "device"|"setup", id: string}`；设备和 setup 的 ID 命名空间不同。`driver_kind` 白名单为 `osa|voltage|gain|pm400|mdt|fiber`。
- `DomainConfig = {domain: DomainRef, config_rev: int, driver_kind: string, model_id: string, profile_id: string|null, params: object, expected_identity: object, members: array}`。独立设备 members 为空；Fiber members 只含本 Host 一侧或两侧已登记 MDT 设备。Host 从注册表生成，GUI 不能直接提交此内部结构。
- `ContextV3 = {session_id: string, domain: DomainRef|null, connection_id: string|null, epoch: int}`。全局请求 domain/connection_id 为 null、epoch=0；预连接域 context 仍带 domain，connection_id 为 null。
- `RequestV3 = {v:3,id:string,method:string,params:object,context:ContextV3|null}`；method 仅 `ping|status|activate|inventory|configure_domain|retire_domain|probe|connect|disconnect|resume|action|shutdown`。只有 ping/status 可以无 context；管理方法使用全局 context 和已验证的 DomainConfig，动作从域解析适配器，不能携带 role/driver_kind 覆盖它。activate 是 Host 内部一次性启动门：params 仅 ownership_nonce，核对启动 nonce 后启用管理/设备入口；此前仅允许 ping/status/shutdown。GUI 无此操作。
- `OutcomeV3` 原样保留 `phase/context/result/error` 与五阶段；`ProbeReport` 是不可变、可 JSON 编码的 `identity/observations/release_confirmed`，失败另附原异常及不可变清理报告。未释放时由 worker 保留驱动对象，不仅保留文字。
- `DeviceRecord = {device_id,name,category,model_id,driver_id,profile_id,params,expected_identity,identity_strength,config_rev,check_policy}`；`SetupRecord = {setup_id,name,kind:"fiber",members,config_rev}`。`DraftRecord` 使用最终 device_id，含 config_digest、revision、mode 和验证责任；取消后 ID 不复用。
- `HostRequest = {ipc_v:1,id:string,method:string,params:object}`；方法白名单 `hello|snapshot|catalog|draft_create|draft_update|draft_verify|draft_save|draft_cancel|device_edit|device_delete|setup_save|settings_get|settings_save|acquire|heartbeat|release|prepare|execute|safe_stop|operation_get|result_read|stop_host`。严格方法专属 schema；会话由可信 IPC 握手建立，不能自报 local/admin。
- `ExecuteParams` 包含 host_id、host_boot_id、client_session_id、domain、config_rev、control_epoch、control_token、context、operation、args、confirm_token、command_seq。重复键为 session+HostRequest.id；同键参数摘要不同拒绝。安全停止使用独立序号域和保留容量。
- `DeviceState` 分离 `communication = ONLINE|OFFLINE|UNKNOWN`、`driver = DISCONNECTED|READY|ACTIVE|BUSY|FAULT|RETAINED|UNKNOWN`、detected、observed_at、age_upper_ms、check_quality、controller、immutable_cleanup 和 output_evidence。输出未知不是零；ONLINE 不等于 READY 或租约。
- `HostEvent = {ipc_v:1,host_id,host_boot_id,event_seq,kind,target,payload}`；kind 为 snapshot/telemetry/operation/lease/directory/fault/host。GUI 键使用 host_id + domain.kind + domain.id；相同型号不共享草稿、图表、恢复状态或证据。

Rust 上述结构在 `host/contracts.rs` 定义并 `deny_unknown_fields`；数值/长度/重复键验证在解码器中执行。Python 相应结构使用 frozen dataclass 和深拷贝；共享测试向量放 `App/tests/fixtures/v3-contracts.json`。正式注册/编辑只接受型号 profile 明确允许的字段。

公共辅助结构同样在 contracts 定义：`Session` 为 OS 认证后的 host_boot_id/client_session_id/user_sid；`Consent` 为 stage、绑定摘要及本机明确接受记录；`ControlGrant` 为可信租约检查返回的 session/domain/control_epoch/context/expiry，不接受客户端自报。`OperationRecord` 包含 operation_id、session/request_id、domain、参数摘要、状态、原始 phase/context、timestamps、result_ref、immutable_cleanup。`HostReply` 包含 ipc_v/id/result 或 code/message error。各任务定义的 Report/State DTO 必须深拷贝且不能携带驱动对象到 GUI；内部资源 owner 保存在 DomainState。CleanupReport 复用 `Code/Setups/session.py` 的既有类型，不在驱动 common 中建立反向依赖。

Task 7 的验证协调器先用可注入责任/控制检查端口完成离线测试；实际 Host 在 Tasks 11–12 连接租约和操作记录后才允许 supervised 验证。未接通端口时明确拒绝，不以测试 stub 作为生产授权。Tasks 1–9 的单元交付无需启动未完成的 Host。

### Task 1: 型号目录与严格连接 profile

**Files:** Create `App/catalog/devices.json`, `App/worker/catalog.py`, `App/src-tauri/src/lib.rs`, `App/src-tauri/src/host/mod.rs`, `App/src-tauri/src/host/contracts.rs`, `App/src-tauri/src/host/catalog.rs`, `App/tests/test_device_catalog.py`; Modify `App/src-tauri/Cargo.toml` 仅登记共享 lib；Test 新 Python 文件及 Rust `host::catalog::tests`。

**Interfaces:** Produces Python `load_catalog(path: Path) -> Catalog`, `Catalog.profile(model_id: str, profile_id: str) -> Profile`, `Profile.validate(params: dict) -> dict`；Rust `Catalog::load(bytes: &[u8]) -> Result<Catalog, HostError>`、`validate_connection(&self, model: &str, profile: &str, params: &Value) -> Result<Value, HostError>`。Catalog/Profile 为目录的不可变视图，HostError 为公共 code/message 错误。

- [ ] **Step 1: 写失败测试**，目录包含 OSA、ESA、Oscilloscope、Function Generator、Power Meter、Piezo Controller、Custom。现有自制电压/增益位于 Custom；未支持型号只能草稿。

```python
def test_profile_does_not_invent_transports(self):
    self.assertEqual(catalog.profile("aq6370", "gpib-visa").access, "visa")
    self.assertRaises(CatalogError, catalog.profile, "aq6370", "tcpip-visa")
    self.assertFalse(catalog.category("ESA").can_register_without_driver)
    self.assertEqual(catalog.profile("gain", "cp210x-serial").baudrate, 115200)
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_device_catalog -v`（P/N/C 的实际命令见文末）；预期缺目录接口 FAIL，不是环境失败。
- [ ] **Step 3: 实现上述接口。** lib/host/mod 仅导出本任务模块，contracts 先提供 HostError 和目录 DTO，后续任务按需扩展。为已核对的 AQ GPIB/VISA、PM400 VISA、三种 115200 串口、Fiber setup 声明固定模型/profile/operations/probe_version；driver_id 经目录映射到 driver_kind，不接收任意 import。物理 interface 与 access 分字段；禁用未经验证的 Ethernet/vendor SDK 路径。USB 芯片只作候选提示。通信证据 TTL 初始统一 90 s，只约束 communication，不替代 Gain 温度新鲜度。OSA/PM400/MDT 标记待 Task 6 审核的 readonly profile；Voltage/Gain 标记 supervised，禁止主动探测。
- [ ] **Step 4: 重跑 Python 测试及** `C test --offline --locked host::catalog`；预期 PASS，并检验 Rust/Python 读取同一目录、拒绝额外参数/非有限数值。
- [ ] **Step 5: 检查点**：目录不声称所有仪器类别已有驱动；记录公开操作及每个 lifecycle 的输出副作用，不提交。

### Task 2: 原子注册表与旧配置迁移

**Files:** Create `App/src-tauri/src/host/registry.rs`; Modify `App/src-tauri/src/host/contracts.rs`, `host/mod.rs`, `Cargo.toml`, `Cargo.lock`, `App/worker/settings.py`（仅保留 v2 旧格式边界）；Test Rust `host::registry::tests`。

**Interfaces:** Consumes Task 1 Catalog。Produces `Registry::open(path: &Path) -> Result<Registry, HostError>`、`commit(&mut self, expected_rev: u64, change: RegistryChange) -> Result<RegistrySnapshot, HostError>`、`migrate_v1(bytes: &[u8]) -> Result<Migration, HostError>`；RegistryChange 为明确 enum（SaveDevice/EditName/RetireDevice/SaveSetup/SaveSettings），不能包含运行状态。

- [ ] **Step 1: 写失败测试** `migration_preserves_original_bytes`、`future_version_blocks_writes`、`concurrent_edit_conflicts`、`atomic_replace_failure_publishes_nothing`。

```rust
assert_eq!(migration.backup_bytes, original);
assert!(migration.devices.is_empty());
assert_eq!(migration.drafts[0].status, "Imported draft");
assert_eq!(registry.commit(stale_rev, change).unwrap_err().code, "ConfigConflict");
assert_eq!(registry.snapshot(), before_failed_replace);
```

- [ ] **Step 2: 运行** `C test --offline --locked host::registry`；预期新模块 FAIL。
- [ ] **Step 3: 实现接口。** 为原子写入和随机 host_id 在本任务加入已缓存 `windows-sys = "=0.61.2"` 的 Win32_Foundation/Win32_Storage_FileSystem/Win32_Security_Cryptography 功能，Task 10 只扩展同一依赖的功能；不联网、不升级版本。文件置于本机应用配置目录，持久 host_id；Host 为唯一写入者。写同目录临时文件、flush/sync，已有文件用 ReplaceFileW、初次创建用 MoveFileExW 原子完成，失败保留旧文件和草稿。v1 先保存原字节备份再迁移 Imported drafts，不生成已登记默认 OSA/空槽位；损坏/未来版本只读封锁。CAS 使用全局 registry_rev 和记录 config_rev；改名不换 ID，物理改绑新 ID、旧 tombstone。删除只能消费后续责任层签发的 confirmed-release 许可。禁止持久 ONLINE、READY、租约或 Fiber 基线。
- [ ] **Step 4: 重跑上列 Rust 测试**，另测磁盘满、备份失败、64/16 超限、setup 悬空引用；预期 PASS。
- [ ] **Step 5: 检查点**：备份、恢复和失败路径可核对，原配置及未知责任不被空默认值覆盖。

### Task 3: worker v3 契约与分版入口

**Files:** Create `App/worker/contracts_v3.py`, `App/tests/test_contracts_v3.py`, `App/tests/fixtures/v3-contracts.json`; Modify `App/worker/main.py`, `protocol.py`; Test 同上及旧 `test_dispatch_contracts.py`, `test_ingress_v2.py`。

**Interfaces:** Produces `parse_v3(line: str) -> RequestV3`、`encode_v3(request_id: str, outcome: OutcomeV3) -> str`、`classify_v3(request: RequestV3, domain_config: DomainConfig|None) -> str`；启动 `--protocol 2|3` 固定解析器，不复用 parse_v2 的 role 回退。

- [ ] **Step 1: 写失败测试** `test_versions_never_fallback`、`test_context_cannot_cross_same_kind_devices`、`test_v3_rejects_duplicate_and_nonfinite_fields`。

```python
self.assertRaises(ProtocolError, parse_v3, v2_line)
self.assertRaises(ProtocolError, parse_v2, v3_line)
self.assertNotEqual(ctx_a.domain, ctx_b.domain)
self.assertRaises(ProtocolError, parse_v3, duplicate_context_line)
self.assertEqual({o.phase for o in outcomes}, set(FIVE_PHASES))
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_contracts_v3 -v`；预期新 API FAIL。
- [ ] **Step 3: 实现接口和共享向量。** v3 请求先限制 64 KiB 再解析，拒绝 bool 当整数、NaN/Inf、重复键、未知方法/实例字段及 priority。启动握手回显协议、模式、session、零既有连接；v3 CLI 增加 `--ownership-nonce <128-bit-hex>`，缺少或错误 nonce 时不启用 activate 后的入口，v2 CLI 不变。configure/retire 只由 Host 内部管理路径生成，不能成为 GUI 原始 RPC。保留 v2 原长度、方法和 reply 契约，不改变旧 fixture 的含义。
- [ ] **Step 4: 运行新测试及两项旧协议模块**；预期 PASS，启动 v3 后送 v2、启动 v2 后送 v3 均拒绝且模拟驱动调用数为 0。
- [ ] **Step 5: 检查点**：文档和向量固定字段/阶段，Rust Task 9 使用同一向量。

### Task 4: 按执行域隔离适配器与安全调度

**Files:** Create `App/worker/domains.py`, `App/tests/test_domains.py`, `test_scheduler_v3.py`; Modify `controller.py`, `scheduler.py`, `safety.py`, `observations.py`, `pm400_ops.py`, `simulation.py`, `main.py`；Test 新模块及旧 scheduler/safety/observations/controller 回归。

**Interfaces:** Consumes RequestV3、DomainConfig。Produces `DomainRegistry.configure(config: DomainConfig) -> ContextV3`、`get(ref: DomainRef) -> DomainState`、`retire(ref: DomainRef) -> None`；`SchedulerV3.submit(request: RequestV3) -> Future[OutcomeV3]`、`context(ref: DomainRef) -> ContextV3`、`status() -> dict`、`join(timeout: float) -> bool`。DomainState 集中保存设备对象、资源、session、Gain evidence/round/last_command、Fiber 端口、清理和取消屏障。

- [ ] **Step 1: 写失败测试** 两台 AQ、Gain、Voltage 分别动作、读回失败、恢复、停止；增加 `test_pre_recovery_status_cannot_clear_unknown_on_another_instance`、`test_revoke_waits_for_final_cleanup_pass`。

```python
self.assertEqual(gain_a.evidence.last_command, "disable_current")
self.assertNotEqual(gain_a.context, gain_b.context)
self.assertEqual(gain_b.driver.calls, before_b)
self.assertFalse(a.is_ready_after(old_status))
self.assertEqual(scheduler.pending_normal_limit, 31)
self.assertEqual(scheduler.reply_slot_limit, 226)
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_domains App.tests.test_scheduler_v3 -v`；预期缺实例核心 FAIL。
- [ ] **Step 3: 提取共同生命周期并实现接口。** v2 包装固定五域，v3 登记动态域；所有 evidence 和 observation 传域引用。固定上限线程池：普通 4、观察 4、安全 64，另 1 个全局管理线程；不随设备增加创建线程。安全等待旧调用不能占用池工作线程；已进入驱动不能硬取消，最终 pass 后才结束责任。每域单写入、总普通 31、域安全最多 3、全局保留查询/关闭、225/226 上限分别在准入和回复处检查。目录变更只允许无旧责任且 confirmed release 的域。MDT 独立适配器只提供配置/只读诊断，Fiber 动作仍走 setup。
- [ ] **Step 4: 新模块及** `P -B -m unittest App.tests.test_scheduler App.tests.test_safety_dispatch App.tests.test_observations App.tests.test_worker_controller -q`；预期全 PASS。注入 64 个阻塞普通域，安全请求仍各有责任，shutdown/状态不被普通容量挤掉。
- [ ] **Step 5: 检查点**：旧 WAITING_OLD、最后关闭 pass、终态保留、f81e674 恢复屏障语义不变；不要只验证空闲停止。

### Task 5: 物理资源预约与 Fiber 复合域

**Files:** Create `App/worker/resources.py`, `App/tests/test_resource_domains.py`; Modify `controller.py`, `domains.py`, `Code/Setups/fiber_coupling.py` 仅增加显式成员筛选入口；Test 新模块及 `Code/Debugs/test_fiber_coupling.py`, `test_visa.py`。

**Interfaces:** Produces `ResourceClaims.reserve(domain: DomainRef, claims: tuple[ResourceClaim,...]) -> Reservation`、`confirm_release(reservation: Reservation, evidence: CleanupReport) -> None`；ResourceClaim 分别保存 canonical VISA、稳定运输身份和 serial port。`FiberCouplingSetup.for_members(serials: tuple[str,...]) -> FiberCouplingSetup` 只接受既定左/右序列号的一侧或两侧，空集合拒绝。

- [ ] **Step 1: 写失败测试** `test_serial_asrl_alias_is_reserved_once`、`test_unresolved_alias_independence_is_rejected`、`test_one_side_setup_uses_serial_not_port_order`。

```python
self.assertRaises(ResourceBusy, claims.reserve, b, alias_of_a)
self.assertEqual(factory.open_count, 1)
self.assertEqual(left_only.stage("left").serial, "2110148249-10")
self.assertRaises(ResourceBusy, claim_raw_mdt_while_setup_owns_it)
self.assertEqual(stage.calls_after_fault, ["hold", "close"])
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_resource_domains -v`；预期 FAIL。
- [ ] **Step 3: 实现接口。** 复用现有 canonical VISA 解析/共享 lease；VISA 名称不随意 casefold，ASRL 与 COM 指向同物理接口时共预约。不能证明独立的候选同时打开拒绝。预约先于打开，清理失败继续占用；外部 owner 与本程序预约分开。setup 为一个域，一次原子预约所有子设备，禁止与裸设备/另 setup 重叠；变更成员先终结旧域责任。左/右和坐标仅由 setup 映射，不在 MDT driver 写入 bench 语义。
- [ ] **Step 4: 重跑新模块和两个既有离线模块**；预期 PASS，测试另一台同型号及不同 VISA 资源仍独立，借用 manager 不提前关闭。
- [ ] **Step 5: 检查点**：一侧可用、对方未连接不阻止登记；setup 不产生额外物理设备计数或直接 MDT 运动入口。

### Task 6: 无输出副作用的公开身份探测

**Files:** Modify `Code/Utils/common.py`, `osa.py`, `pm400.py`, `mdt693b.py`; Create `Code/Debugs/test_identity_probe.py`; Test 同上及 `test_osa_lifecycle.py`, `test_pm400.py`, `test_mdt693b.py`。

**Interfaces:** Produces frozen `ProbeReport(identity: dict, observations: dict, release_confirmed: bool)` 和各 driver `probe_identity(self) -> ProbeReport`。调用期间对象可保持失败责任；失败异常不能使调用者丢弃仍持有资源的 driver。

- [ ] **Step 1: 写失败测试** `test_osa_probe_does_not_abort_front_panel_scan`、`test_probe_close_failure_retains_owner`、`test_serial_profile_discloses_open_reset_risk`。

```python
self.assertEqual(osa_transport.queries, ["*IDN?"])
self.assertEqual(osa_measurement.abort_calls, 0)
self.assertEqual(settings_after, settings_before)
self.assertFalse(failed_probe.cleanup.resource_released)
self.assertTrue(failed_probe.driver.has_resource_responsibility)
```

- [ ] **Step 2: 运行** `P -B -m unittest Code.Debugs.test_identity_probe -v`；预期新探测 FAIL。
- [ ] **Step 3: 实现公开探测。** OSA 通过驱动自身 lease/reservation 查询身份，不创建会在普通 close 中 abort 的 measurement 对象；只释放探测 transport，正常 close 行为不变。PM400 只用既有 ID/探头身份读和正常 close；MDT 只读 product/serial/status，保持所有输出且无 motion authority。所有临时生命周期使用 finally；release 失败保留对象与不可变报告。审阅 DTR/RTS/reset/握手/失败关闭行为，把 profile 无法证明的风险标为需人工确认；禁止自动探测。Voltage/Gain 不新增伪只读 connect，仍 supervised/manual verification。
- [ ] **Step 4: 重跑新模块及三个旧模块**；预期 PASS。命令 spy 禁止 reset/zero/TEC/current enable/箭头写入，MDT 非零输出保持，OSA 扫描未被中止。
- [ ] **Step 5: 检查点**：profile 的 readonly eligibility 与公开探测版本一起更新；连接成功不能证明整台设备所有功能已实测。

### Task 7: Test Connection 责任与正式登记

**Files:** Create `App/worker/verification.py`, `App/src-tauri/src/host/verification.rs`, `App/tests/test_registration_probe.py`; Modify worker `domains.py`, `main.py`；Test 新模块和 Rust `host::verification::tests`。

**Interfaces:** Consumes Registry、Catalog、probe_identity、ResourceClaims。Produces worker `Verification.run(config: DomainConfig, authorization: dict) -> ProbeEvidence`；Host `Verifier::begin(draft_id: &str, expected_rev: u64, consent: Consent) -> Result<OperationRecord, HostError>`、`save(draft_id: &str, proof_id: &str, expected_rev: u64) -> Result<DeviceRecord, HostError>`。ProbeEvidence 固定 draft/domain、model/profile、config_digest、config_rev、identity、mode、probe_version、issued_monotonic、release 或 supervised session/controller。

- [ ] **Step 1: 写失败测试** `test_probe_proof_expires_at_60_seconds`、`test_save_failure_keeps_supervised_session`、`test_simulate_proof_cannot_register_real_device`。

```python
self.assertFalse(proof.valid_at(issued + 60.0))
self.assertRaises(StaleProof, save_after_config_change)
self.assertFalse(directory.contains(draft.device_id))
self.assertTrue(failed_save.owner.is_retained)
self.assertEqual(cancelled_draft.reused_id_count, 0)
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_registration_probe -v` 与 `C test --offline --locked host::verification`；预期 FAIL。
- [ ] **Step 3: 实现接口。** 草稿预分配最终 ID 并占域/资源责任容量，临时只读成功必须确认 release 才签 60 s proof；失败/未释放不能另开重试。Voltage/Gain 显示 Manual verification required，独立 supervised 流程先准备并逐项确认生命周期副作用，再由 driver 执行；不得由 Add and Save 发起 connect。保留 supervised 会话时必须已有合法控制者、租约及明确 retained 标记；配置写失败仍受 Task 11 失权清理。先原子写注册表再发布目录；cancel 先清理再退役。型号、参数、身份、模式/版本任何变化失效，不能拿枚举结果当 proof。
- [ ] **Step 4: 重跑两组测试**，加 identity mismatch、proof 重用、draft 容量满、close 超时、写盘失败及退出 wizard 清理失败；预期 PASS，不丢失资源 owner。
- [ ] **Step 5: 检查点**：失败保存≠失败探测，模拟≠真实证据；GUI 后续只调用这些接口。

### Task 8: 授权分级的在线检查

**Files:** Create `App/worker/availability.py`, `App/src-tauri/src/host/availability.rs`, `App/tests/test_availability.py`; Modify `observations.py`, `domains.py`; Test 新模块及 Rust `host::availability::tests`。

**Interfaces:** Produces `Availability.tick(now: float) -> list[CheckIntent]`、`complete(intent: CheckIntent, evidence: ProbeEvidence|Observation) -> DeviceState|None`；Host `CheckPolicy::authorize(stage: CheckStage, binding: CheckBinding) -> CheckPolicy`。持久 CheckBinding 绑定 mode/identity/config_rev/profile/probe_version，枚举、readonly 是不同 consent；每次 CheckIntent 另固定当前 boot、domain 和 connection context，发布前重新匹配，不能把启动身份写进永久许可导致每次重启错误失效。

- [ ] **Step 1: 写失败测试** `test_default_start_does_not_open_ports`、`test_busy_probe_deferred_without_offline_claim`、`test_late_success_cannot_mark_rebound_device_online`。

```python
self.assertEqual(startup_without_consent.open_calls, 0)
self.assertEqual(backoff_delays, [30, 60, 120, 300, 300])
self.assertEqual(availability.max_active_idle_probes, 1)
self.assertEqual(busy_state.communication, "UNKNOWN")
self.assertIsNone(availability.complete(old_intent, success))
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_availability -v`；预期 FAIL。
- [ ] **Step 3: 实现接口。** 30 s 周期、10 s 下限，启动按登记顺序错开；每设备最多一个、Host 单独探测同时一个。已 owned 的 driver 只用合并巡检/cache，不新开端口；普通 busy/cleaning/外部占用分开质量字段。detected 不是 ONLINE，通信 evidence TTL 90 s 后未知/过期而非输出 Off。profile 或配置变化立即撤销旧 policy 许可，重新插入需稳定身份复核；不自动 COM 改绑。source/gain 仅枚举或 owned evidence，never background connect。GUI 数量不增加巡检，watchdog 保持原周期。
- [ ] **Step 4: 新模块与 Rust 策略测试 PASS**；注入 DTR 风险、无授权、端口占用、设备重插、暂停 UI、64 个启动目标，确认无多余线程或主动探测积压。
- [ ] **Step 5: 检查点**：检查许可不是租约，不建立输出或基线权限。

### Task 9: 提取共享 Rust runtime 并接入 v3

**Files:** Create `App/src-tauri/src/runtime.rs`; Modify `lib.rs`, `worker.rs`, `reply_broker.rs`, `request_writer.rs`, `startup_handshake.rs`, `worker_root.rs`, `Cargo.toml`; Modify `App/tests/native_worker_fixture.py`, `test_dispatch_e2e.py`; Test Rust 模块及 Python fixture。

**Interfaces:** Produces `WorkerRuntime::spawn(config: RuntimeConfig) -> Result<Arc<WorkerRuntime>, RuntimeError>`、`submit(request: WorkerRequest) -> Result<PendingReply, RuntimeError>`、`stop() -> Result<StopReport, RuntimeError>`；RuntimeConfig 固定 python/root/mode/protocol/ownership_nonce/启动记录钩子，不含 GUI handle。WorkerRequest 为 v2/v3 enum；broker v3 索引完整 domain/context，v2 仍旧 role。v3 spawn 核验握手后调用记录钩子，成功才发送 activate；钩子失败不返回可用 runtime，并保留 child 责任与启动记录。

- [ ] **Step 1: 写失败测试** `v3_wrong_domain_reply_cannot_clear_restriction`、`ordinary_saturation_preserves_192_safety_slots`、`handshake_requires_requested_version`。

```rust
assert_eq!(runtime.protocol(), 3);
assert!(broker.deliver(reply_for_other_domain).is_err());
assert_eq!(writer.normal_capacity(), 31);
assert_eq!(broker.liability_capacity(), 225);
assert!(!stop_report.claims_physical_zero());
```

- [ ] **Step 2: 运行** `C test --offline --locked`；预期新增测试 FAIL，原测试基线不应改变。
- [ ] **Step 3: 移动而非复制运行时。** worker.rs 暂保留旧 Tauri adapter 供 v2 回归，独立 Host 调用同一个 runtime。五阶段、timeout 后 liability、晚到 cleanup、writer 安全优先及 shutdown 最终 pass 全保留。版本和容量分开：v2 原数值，v3 225 pending/226 reply；旧 context 只作历史，不清新域 restricted。启动握手核验协议/mode/no connected，Python 路径父目录必须 VISA。v3 worker 回复读取限制 17 MiB（16 MiB 结果加 framing）；超限结果为明确容量错误，不能截断或无限 readline。
- [ ] **Step 4: Rust 全测试与** `P -B -m unittest App.tests.test_dispatch_e2e App.tests.test_worker_process -q`；预期 PASS。共享向量两语言同判，晚回复与进程退出的证据仍分开。
- [ ] **Step 5: 检查点**：runtime 不依赖 Tauri AppHandle，也无第二个 spawn/强杀实现；正常 GUI 入口在 Task 13 切换前保持兼容。

### Task 10: 独立 Host 与受保护本机 IPC

**Files:** Create `App/src-tauri/src/bin/yang-lab-host.rs`, `host/instance.rs`, `host/ipc.rs`, `host/service.rs`, `host_client.rs`; Modify `host/mod.rs`, `Cargo.toml`, `Cargo.lock`; Test Rust host IPC/instance 模块及 `App/tests/test_local_host_process.py`（新建）。

**Interfaces:** Produces `HostService::start(config: HostConfig) -> Result<HostService, HostError>`、`serve(self) -> Result<(), HostError>`、`InstanceGuard::acquire(record_dir: &Path) -> Result<InstanceGuard, HostError>`；`LocalHostClient::connect(endpoint: &str) -> Result<LocalHostClient, HostError>`、`call(request: HostRequest) -> Result<HostReply, HostError>`、`subscribe() -> HostEvents`。HostConfig 由可信本机设置构造，模式固定到停止。

- [ ] **Step 1: 写失败测试** `second_host_fails_without_spawning_worker`、`pid_reuse_is_not_liveness_proof`、`pipe_rejects_untrusted_and_remote_clients`。

```rust
assert_eq!(second_start.unwrap_err().code, "HostAlreadyRunning");
assert_eq!(spawn_count(), 1);
assert_eq!(orphan_start.unwrap_err().code, "OwnershipUnknown");
assert!(security.client_mask & FILE_CREATE_PIPE_INSTANCE == 0);
assert_eq!(oversized_frame.accepted_worker_calls, 0);
```

- [ ] **Step 2: 运行** `C test --offline --locked host::ipc` 与 `host::instance`；预期 FAIL。
- [ ] **Step 3: 实现接口。** 增加锁中已缓存 `tokio = "=1.53.1"`（net/rt-multi-thread/sync/time/io-util）和 `windows-sys = "=0.61.2"` 必要 Windows 功能，不升级其它依赖或联网下载。windows-sys 功能明确为 Win32_Foundation、Win32_Security、Win32_Security_Authorization、Win32_Security_Cryptography、Win32_Storage_FileSystem、Win32_System_Pipes、Win32_System_Threading、Win32_System_IO、Win32_System_WindowsProgramming、Win32_UI_WindowsAndMessaging、Win32_UI_Shell。命名管道为本地 user SID 派生路径，显式 DACL 只授当前 Windows 用户所需读/写数据，不用默认 DACL/FILE_GENERIC_WRITE；拒绝远程客户端。tokio ServerOptions 的首实例用 first_pipe_instance(true)，后续同管道实例不用此旗标；create_with_security_attributes_raw 配合 owned 安全描述符，异步使用 overlapped，不用 PIPE_NOWAIT 轮询。客户端用 CreateFileW 明确申请 FILE_READ_DATA/FILE_WRITE_DATA、必要属性与同步权限，不申请 FILE_APPEND_DATA/FILE_CREATE_PIPE_INSTANCE，再交 NamedPipeClient::from_raw_handle；不能用会申请 GENERIC_WRITE 的 ClientOptions::open 破坏权限设计。[Windows 管道安全](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)，[首实例与远程拒绝旗标](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea)。

  机器级 Global mutex 防合作程序多 Host，AccessDenied/已有实例均封锁；核对残存 worker 的 PID、进程创建时间和启动 nonce，未知绝不 auto-kill。先落盘 startup intent，再 spawn；子进程在 ownership activation 前禁止任何硬件入口，Host 记录 child 身份后才 activation。启动窗口崩溃留下未解析 intent 时封锁新 spawn，不推测安全。Host/client 最大 16，长度前缀普通帧上限 64 KiB，重复键/单位/ID/版本严格校验；配置管理由 OS 检查后的本机会话提供，消息 local=true 无效。无监听网络端口，无自启动服务。
- [ ] **Step 4: 上列 Rust 测试及** `P -B -m unittest App.tests.test_local_host_process -v`；预期 PASS。模拟启动/关闭、管道重连、16→17 客户端、截断帧、不同用户访问安全描述符、Host 崩溃残存 worker 均 fail closed。测试 harness finally 仅正常停止已创建模拟进程；不杀未知进程。
- [ ] **Step 5: 检查点**：本机 IPC 正常不依赖网络；命名空间/端口冲突不能触发第二套 worker。

### Task 11: 单写控制权与失效清理

**Files:** Create `App/src-tauri/src/host/leases.rs`; Modify `host/service.rs`, `runtime.rs`; Create `App/tests/test_local_leases.py`; Test Rust `host::leases::tests` 与新模拟集成。

**Interfaces:** Produces `LeaseBook::acquire(session: &Session, domain: &DomainRef, now: Instant) -> Result<Lease, HostError>`、`renew(token: &str, session: &Session, now: Instant) -> Result<Lease, HostError>`、`revoke(domain: &DomainRef, cause: RevokeCause) -> CleanupTicket`。Lease 绑定 boot/session/domain/control_epoch，安全随机 token；CleanupTicket 保留责任至确认 release。

- [ ] **Step 1: 写失败测试** `two_clients_have_one_writer`、`expired_lease_fences_before_dispatch`、`sleep_resume_revokes_without_restoring_baseline`。

```rust
assert_eq!(successful_acquisitions, 1);
assert!(lease.admit_at(start + Duration::from_secs(10)).is_err());
assert_eq!(other_domain.cleanup_count, 0);
assert!(old_context.is_fenced());
assert!(!fiber_state.baseline_valid);
```

- [ ] **Step 2: 运行** `C test --offline --locked host::leases`；预期 FAIL。
- [ ] **Step 3: 实现接口。** acquire 只给软件权利，不 connect/resume/输出。2 s GUI 心跳、10 s Host 单调期限，每次准入复核；事件不可消费/关闭 GUI 不续期。revoke 与普通 dispatch 在同域排序，先推进 control_epoch、封锁 Host 旧请求，再 worker disconnect 代次屏障。已开始的调用等待最终清理，不承诺即时物理停止；失败保持 RETAINED 且禁止下一租约。锁不跨驱动等待、发送或磁盘 I/O。Windows 休眠恢复事件和时钟异常撤销全旧租约，不延长旧期限。close GUI 只撤销自己域，safe_stop 有明确本机会话权利并撤销域，stop_host 才全局关闭。
- [ ] **Step 4: Rust 测试与** `P -B -m unittest App.tests.test_local_leases -v`；预期 PASS。分别在 Host 未发送、worker 排队、驱动已开始三位置失权； source 零、gain 电流先 TEC、Fiber hold、不干扰其它域。
- [ ] **Step 5: 检查点**：两个本机 GUI 无优先特权或强行 takeover；普通 connect 不错误要求 READY，普通 action 仍要求驱动许可状态。

### Task 12: 一次性确认与持久操作责任

**Files:** Create `App/src-tauri/src/host/operations.rs`; Modify `host/contracts.rs`, `host/service.rs`; Test Rust `host::operations::tests` 及新 `App/tests/test_operation_ledger.py`。

**Interfaces:** Produces `OperationBook::prepare(session: &Session, intent: ExecuteParams, now: Instant) -> Result<Confirmation, HostError>`、`admit(session: &Session, request_id: &str, intent: ExecuteParams, now: Instant) -> Result<OperationRecord, HostError>`、`find(session: &Session, id: &str) -> Result<OperationRecord, HostError>`；确认/租约 token 至少 128 位随机，hash/digest 使用 Windows CNG SHA-256 对规范参数编码，内部 worker ID 为 ≤64 字符独立摘要。规范参数编码由 Host 统一递归排序对象键、无空白 UTF-8 JSON；worker 不自行重新计算授权摘要。

- [ ] **Step 1: 写失败测试** `duplicate_does_not_consume_confirmation_twice`、`disk_full_rejects_normal_but_not_safe_stop`、`restart_preserves_unknown_without_replay`。

```rust
assert_eq!(duplicate.operation_id, first.operation_id);
assert_eq!(driver_calls(), 1);
assert_eq!(changed_args.unwrap_err().code, "RequestConflict");
assert_eq!(journal_failure.worker_calls, 0);
assert!(safe_stop_after_disk_full.is_admitted());
assert_eq!(restarted_record.status, "Outcome Unknown");
```

- [ ] **Step 2: 运行** `C test --offline --locked host::operations`；预期 FAIL。
- [ ] **Step 3: 实现接口。** 准备仅 cache/参数校验，60 s 一次性 token 绑定设备、连接、安全和权限代次；普通遥测不无故失效，目标/安全改变则失效。先 OS 会话/权限，再同 session+id 去重和摘要核对，再租约/确认/目标/容量；建立 durable accepted 记录后才提交。普通记录落盘失败拒绝前调用；安全入口用独立保留容量，存储失败报告审计故障但仍清理。Host 准入 5 s 仅收据等待，超时查询旧 ID、不自动重发。消息序号严格增长、普通串行准入、安全独立水位；4096/24 h 终态淘汰后旧序号 Record Expired，非终态不丢。重启 boot/session/token 全换，未终态未知无 replay。把真实 LeaseBook/OperationBook 接入 Task 7 的验证协调器，连接阶段 consent 和 supervised 输出副作用确认分开核验；此时才打开该生产入口。
- [ ] **Step 4: Rust 测试与** `P -B -m unittest App.tests.test_operation_ledger -v`；预期 PASS。确认恰好 60 s、跨域/跨 boot、参数变更、非法 PM 维护授权、丢收据及索引淘汰不得产生第二次动作。
- [ ] **Step 5: 检查点**：已接管≠已执行、completed≠物理输出证明；五阶段与不可变 cleanup 留在日志。

### Task 13: 一致快照、有界事件和实例 GUI 客户端

**Files:** Create `host/events.rs`, `host/results.rs`, `App/web/host-client.js`, `device-store.js`, `routes.js`, `App/tests/device-store.test.mjs`, `host-client.test.mjs`; Modify `main.rs`, `host_client.rs`, `App/web/api.js`, `control-state.js`, `main.js`, `lifecycle.js`, `operations.js`, `view-model.js`, `App/tests/preview-bridge.js`; Test 新 Node 与 Rust模块，保留旧 api/v2 fixture 回归。

**Interfaces:** Produces `EventHub::subscribe(session: &Session) -> Result<SnapshotStream, HostError>`、`publish(event: HostEvent) -> Result<(), HostError>`、`ResultStore::read(session: &Session, id: &str, offset: u64, length: u32) -> Result<ResultChunk, HostError>`；JS `createHostClient(invoke, listen) -> HostClient`、`createDeviceStore() -> DeviceStore`、`routeFor(hostId, domain) -> Route`、`store.apply(event) -> ApplyResult`。HostClient 提供与 HostRequest 对应的命名方法，不暴露 raw worker_request。

- [ ] **Step 1: 写失败测试** `snapshot_then_events_has_no_gap`、`slow_client_releases_only_its_domains`、`old_instance_result_is_history_not_current`。

```javascript
assert.equal(store.get(keyA).gain.lastCommand, 'disable_current');
assert.equal(store.get(keyB).gain.lastCommand, 'unchanged');
assert.equal(store.apply(oldEpochEvent).currentChanged, false);
assert.equal(store.apply(sequenceGap).requiresSnapshot, true);
assert.equal(store.canControl(keyA), false);
```

- [ ] **Step 2: 运行** `N --test App/tests/device-store.test.mjs App/tests/host-client.test.mjs`；预期缺接口 FAIL。
- [ ] **Step 3: 实现接口并切换生产入口。** Host 原子订阅/快照边界，目录与序号一致但不假称测量同时采样。事件256/1MiB，遥测按完整 key 合并；责任队列满断开慢客户端、触发其租约清理。一个 Host 实际 status 查询约 2.5 s，心跳独立通道。光谱只发 result ID/size/checksum/acquisition，结果≤16MiB、chunk≤256KiB，读取独立队列、每次检查 session/域；普通控制帧64KiB限制不套在该只读 result reply 上。结果文件默认 Result/console/host_id，下载参数无文件路径。

  Tauri handlers 改成 host_connect/call/subscribe/disconnect 和本机 start/stop，生产 GUI 不再获得 WorkerState；v2 测试包装器保留。JS 所有 state、草稿、gain guard、图表、cancel/resume 按 host+domain key；旧 context 只能历史显示。seq缺口/boot变化先封锁、重同步，不续旧租约。freshness 使用 Host sample age 上界、IPC clock ping 与客户端接收增长；无法给上界标 Freshness Unknown，GUI 和 Host 均禁止依赖该字段的动作。客户端断线输出未知而非 Off/零；只导航不执行任何 RPC。
- [ ] **Step 4: 新 Node、Rust events/results 与所有旧 Node 测试**；预期 PASS。两 GUI 同操作ID/五阶段/读回，增加 GUI 不增加轮询；大结果/慢GUI不堵安全、心跳或 shutdown。
- [ ] **Step 5: 检查点**：删除生产的 GUI→worker 旁路；模拟桥严格 v3，也不得打开真实仪器。

### Task 14: Add New、Host Settings 和 Fiber Setup 界面

**Files:** Create `App/web/setup.js`, `App/tests/device-setup.test.mjs`; Modify `panels.js`, `main.js`, `index.html`, `pm400.js`, `scene/panel.js`, `App/tests/main.test.mjs`, `operations.test.mjs`, `pm400-ui.test.mjs`；Test 新及修改 Node 文件。

**Interfaces:** Consumes HostClient、DeviceStore、Catalog。Produces `renderDeviceSetup(state) -> string`、`renderAddWizard(draft, catalog) -> string`、`renderHostSettings(host) -> string`、`renderSetup(setupKey, store) -> string`；事件 handler 仅调用命名 HostClient 方法，render 无调用副作用。

- [ ] **Step 1: 写失败测试** `wizard_requires_current_proof_before_save`、`unsupported_model_stays_draft`、`fiber_controller_routes_to_owning_setup`。

```javascript
assert.equal(canSave(expiredProof), false);
assert.match(renderedUnsupported, /Driver required/);
assert.match(renderedUnsafeProbe, /Manual verification required/);
assert.equal(connectCallsFromSaveClick, 0);
assert.equal(mdtPage.hasRawMotionControls, false);
```

- [ ] **Step 2: 运行** `N --test App/tests/device-setup.test.mjs`；预期 FAIL。
- [ ] **Step 3: 实现接口和 handler。** Device Setup 所有物理记录等价，toolbar Add New，Category→Manufacturer and Model→Connection→Test Connection→Add and Save。profile 控制参数字段，枚举/readonly/监督副作用分别确认；名称、强弱身份、release失败/CAS冲突可展开。Host Settings 收纳 VISA路径、simulate/real、显式 Start/Stop；mode变更需关闭重启，无 worker技术卡占主要设备区。setup选择本机一侧或两侧MDT，按 serial映射，裸MDT仅身份/配置入口。各型号能力仍通过现有面板公开操作；PM能力禁用、Gain联锁、Fiber名义估计和基线确认不丢。空清单不填默认假卡，未支持型号不能 Save 成正式仪器。
- [ ] **Step 4: 新测试及修改面板/操作测试**；预期 PASS。验证字段变更需重测、cancel retained不可丢、模拟标签常显、save失败仍可安全处理已保留会话。
- [ ] **Step 5: 检查点**：功能分类不承担唯一槽位；说明精简但输出副作用和未知提示仍可见。

### Task 15: 白色 Overview、精确导航与窗口生命周期

**Files:** Create `App/web/overview.js`, `App/tests/overview.test.mjs`, `window-lifecycle.test.mjs`; Modify `index.html`, `style.css`, `panels.js`, `main.js`, `lifecycle.js`, `scene/viewer.js`, `scene/panel.js`, `App/src-tauri/src/main.rs`, `host/service.rs`; Test 新模块及旧 scene/UI/native close fixture。

**Interfaces:** Produces `overviewCounts(devices, setups, hostStates) -> OverviewCounts`、`renderOverview(store) -> string`、`closeClient(sessionId) -> Promise<CloseReport>`、`stopLocalHost(confirmToken) -> Promise<StopReport>`；CloseReport 区分 client closed 与每域 cleanup，不能以 Host存活代替release。

- [ ] **Step 1: 写失败测试** `counts_physical_devices_not_types_or_setup`、`tag_navigation_never_opens_hardware`、`observer_close_does_not_stop_host`、`failed_controller_close_keeps_window`。

```javascript
assert.equal(overviewCounts(twoMdts, [fiber], hosts).local.total, 2);
assert.equal(navigateTag(cardB).deviceId, cardB.deviceId);
assert.equal(driverCallsAfterNavigation, 0);
assert.equal(observerClose.globalShutdownCalls, 0);
assert.equal(failedControllerClose.windowMayClose, false);
```

- [ ] **Step 2: 运行** `N --test App/tests/overview.test.mjs App/tests/window-lifecycle.test.mjs`；预期 FAIL。
- [ ] **Step 3: 实现接口。** 删除 LOCAL SESSION 装饰横幅与长说明；顶部 Local Devices、Remote Devices实体计数/Host状态，第一阶段remote为0并说明不可用，不展示成功配对假状态。下面真实设备实例卡，LOCAL/REMOTE来源与ONLINE/OFFLINE/UNKNOWN通信分开，驱动/控制者/年龄另列。name/tag/Open Instrument 可键盘导航精确实例；离线可看、操作disabled，MDT卡直达owning setup。白/浅灰/深灰/teal覆盖inputs/selects/toasts/plots/trends/Three背景；故障未知文字、focus和必要确认保留。仪器可辨识模型保留，已要求删除的Overview三维工作台总览不重新插入。

  无租约GUI关闭仅结束客户端；有租约先清理自己的域，失败保留窗口/报告/管理入口。Stop Local Host独立确认列出所有影响设备和控制者，封锁全Host后安全关闭；未release不能退出Host或强杀worker。Windows休眠恢复、事件断线和窗口崩溃均接Task11失权；确认时不把操作停止与物理停止等同。
- [ ] **Step 4: Node 全测试和** `P -B -m unittest App.tests.test_native_close_fixture -q`；预期 PASS。原生模拟窗口人工检查所有页面英文/浅色、1440×900与960×680滚动/focus/label；网页preview只能辅助。
- [ ] **Step 5: 检查点**：无虚构REMOTE、固定五类分母或点击即connect；保存原生验收截图到 Result/console，不放Utils。

### Task 16: 持久运行入口、打包与完整离线验收

**Files:** Create `App/src-tauri/src/host/tray.rs`, `App/tests/test_host_package.py`, `test_local_host_acceptance.py`, `App/scripts/build-host.ps1`; Modify `Cargo.toml`, `tauri.conf.json`, `worker_root.rs`, `App/README.md`, `App/tests/test_package_layout.py`；Test 新模块及全部现有回归。

**Interfaces:** Produces `HostTray::install(host_id: &str, status: HostStatus, reopen: ReopenTarget) -> Result<HostTray, HostError>`、`resolve_packaged_host(resource_dir: &Path) -> Result<PathBuf, HostError>`；build-host.ps1 参数 `-Offline`，仅使用D盘现有Rust/MSVC缓存，生成Tauri externalBin所需target后缀文件。

- [ ] **Step 1: 写失败测试** `test_missing_sidecar_never_uses_source_fallback`、`test_host_status_remains_visible_after_gui_close`、`test_two_gui_multi_instance_acceptance`。

```python
self.assertEqual(missing_host.error.code, "HostPackageMissing")
self.assertEqual(missing_host.worker_spawn_count, 0)
self.assertTrue(host.tray.has_reopen_management)
self.assertEqual(b.gain.calls, before_b_calls)
self.assertEqual(a_gui.operation_id, b_gui.operation_id)
```

- [ ] **Step 2: 运行** `P -B -m unittest App.tests.test_host_package App.tests.test_local_host_acceptance -v`；预期 FAIL。
- [ ] **Step 3: 实现打包和运行标识。** 增加yang-lab-host bin，GUI保持default-run；sidecar在固定resources位置，不接受GUI任意路径。release缺包即失败，worker源码fallback仅debug原规则；不嵌入Python，VISA外部路径明确。helper使用隐藏窗口/CREATE_NO_WINDOW；Host以tray常驻显示mode/运行状态/Open Console/受确认Stop，GUI退出后可重新管理，不能隐形运行。tray停止失败继续保留管理状态。第二次GUI连接既有Host，不重复worker；真实模式不自动激活仪器。README记录三阶段硬件授权、readonly与supervised区别、状态含义和第一阶段remote不可用。
- [ ] **Step 4: 运行文末 V1–V5 与新验收模块**；预期全部PASS且离线build成功。安装包原生启动两个GUI、六个同类型模拟实例和单/双侧Fiber，执行清单；64域/31普通/192安全/慢客户端/16MiB结果压力测试在模拟fixture完成。每个进程正常关闭有报告，未知retain不强杀。缓存缺依赖/工具失败时报告，不下载或调整系统配置掩盖。
- [ ] **Step 5: 检查点**：提供实际修改、测试计数/失败、构建输出及原生截图；经独立审查和用户验收后才能决定下一阶段。不宣称Tailscale/真实仪器/远程网络已可用，不合入main。

## 验证命令

所有命令从实际 App checkout 根目录运行。P/N/C 是文中简写，不是新脚本或 shell alias：执行时替换为下面绝对路径。命令为计划中的验收要求，尚不代表本轮已经运行产品测试。

```powershell
$taskPython = 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe'
$taskNode = 'C:/Users/PIC_YangLab/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node.exe'
$taskCargo = 'D:/Qian/Codex_Project/SIL_Experiments/tmp/tauri-build/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/cargo.exe'
```

V1 Python App 全回归（所有设备工厂为模拟）：

```powershell
& $taskPython -B -m unittest discover -s App/tests -p 'test_*.py' -q
```

V2 驱动/setup 离线回归（不得运行真实诊断入口）：

```powershell
& $taskPython -B -m unittest Code.Debugs.test_drivers Code.Debugs.test_voltage_evidence Code.Debugs.test_osa_lifecycle Code.Debugs.test_pm400 Code.Debugs.test_mdt693b Code.Debugs.test_fiber_coupling Code.Debugs.test_session Code.Debugs.test_visa -q
```

V3 全 Node 回归（PowerShell 不将 glob 原样交外部 exe）：

```powershell
$taskNodeTests = @(rg --files App/tests -g '*.test.mjs')
& $taskNode --test $taskNodeTests
```

V4 Rust 离线测试/格式与 release 打包（先加载现有 MSVC）：

```powershell
& 'D:/SoftwareInstaller/MicrosoftBuildTools2022/Common7/Tools/Launch-VsDevShell.ps1' -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
$taskBuildRoot = 'D:/Qian/Codex_Project/SIL_Experiments/tmp/tauri-build'
$env:RUSTUP_HOME = "$taskBuildRoot/rustup"
$env:CARGO_HOME = "$taskBuildRoot/cargo"
$env:CARGO_TARGET_DIR = "$taskBuildRoot/target"
$env:TEMP = "$taskBuildRoot/temp"
$env:TMP = $env:TEMP
$env:CARGO_NET_OFFLINE = 'true'
$env:PATH = "$taskBuildRoot/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin;$env:CARGO_HOME/bin;$env:PATH"
& $taskCargo test --offline --locked --manifest-path App/src-tauri/Cargo.toml
& $taskCargo fmt --check --manifest-path App/src-tauri/Cargo.toml
& './App/scripts/build-host.ps1' -Offline
Push-Location -LiteralPath App/src-tauri
try { & $taskCargo tauri build --ci -- --locked } finally { Pop-Location }
```

Tasks 2、10 新增/扩展直接依赖时允许对应离线机械更新 Cargo.lock 根引用，随后验收均 locked。保持现有依赖版本，缺缓存停止报告。任务局部 Rust 测试同样使用 manifest-path；每命令检查 LASTEXITCODE，不让后续成功掩盖失败。

V5 原生模拟验收：明确由 Host Settings 选 Offline simulation 并显式启动；验证worker启动参数 `--simulate --protocol 3 --ownership-nonce <本次随机值>`，模拟driver spy确认真实资源调用0。必须通过已安装原生GUI，不以浏览器preview代替。

## 第一阶段验收与设计覆盖

| 已批准设计验收编号 | 第一阶段负责任务与边界 |
| --- | --- |
| 1–2 | 两本机GUI共享同Host由11/13/16验证；跨电脑A/B追加与双向同步留第二阶段，不能声称完成 |
| 3、16 | 同Host同型号隔离4/5/13/16；不同Host相同资源地址独立将在第二阶段网络验收 |
| 4–8 | 本机单写、错误身份、失权三位置、迟到/恢复屏障3/4/9/11/12/13；远程认证撤销留第二阶段 |
| 9–12 | 队列饱和、慢GUI、结果分块、操作记录/重启、单GUI关闭和安全顺序4/9/11/12/13/15/16 |
| 13 | 本机管道/旧Host/worker冲突10/16；证书、配对、代理、远程端口/明文回退留第二阶段 |
| 14–15 | 英文原生/模拟/证据区分14/15/16；两个网络Host及Tailscale实接和真实硬件仍未授权 |
| 17–19 | 型号目录、proof、迁移、原子保存、检查许可和副作用1/2/5/6/7/8/14 |
| 20–22 | 本地实体计数、状态分离、精确导航、离线查看、浅色英文13/14/15；远程许可撤回页面封锁第二阶段 |
| 23 | v2/v3分版、实例/证据/饱和、旧调用等待与终态/退出回归3/4/9/16 |

第一阶段额外必须证明：关闭观察GUI不影响控制者；关闭控制GUI只清理其域且失败留窗；GUI都退出时Host仍可被管理；两个同时Start只能一个Host/worker；无许可启动不打开任何仪器。全部测试报告把软件终态、release证据、Host读回与物理测量分开。

## 审查与执行方式

计划先由用户审阅，再选择直接执行或逐任务子代理执行。当前没有授权派发子代理或实施代码。建议直接执行：协议、实例调度和GUI状态接口耦合较紧，保留上下文有助于保护已有未提交改动；完成后由独立审查者检查全变更，再做用户原生验收。选择子代理方案则每任务实施后独立检查，仍使用同一安全边界和检查点。

第二阶段另写计划，才加入HTTPS/WSS、设备授权过滤、配对/证书/Windows凭据保护、跨Host时钟年龄、网络结果入口和Tailscale部署。第一阶段验收通过不会自动授权第二阶段代码或真实硬件操作。
