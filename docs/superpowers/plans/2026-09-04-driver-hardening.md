# Driver Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复三个已复现的安全/生命周期缺陷，提供可复用的会话清理与真实归零证据，并记录离线验证和实机验收边界。

**Architecture:** 保留各仪器的协议和安全状态机；共享 VISA 层负责管理器租约和资源占用，会话层仅调用驱动公开接口。先修基础层，再迁移实验协调代码；不得重写扫描、存储和分析算法。

**Tech Stack:** Windows / PowerShell，Anaconda VISA，Python 3.10.16，unittest，PyVISA 1.14.1，PyMeasure 0.15.0，pyserial 3.5，NumPy 2.2.4，Matplotlib 3.10.1。

**Spec:** `docs/superpowers/specs/2026-09-04-driver-hardening-design.md`，用户于 2026-09-04 确认。

## Global Constraints

- 所有 Python 命令均在 VISA 环境执行；测试禁用向 Code/Utils 写入字节码缓存。
- 不枚举、不连接、不操作实际仪器，不修改 Anaconda VISA 环境，不安装或升级依赖。
- 不改实验参数和结果，不改 reference_code，不建立 Tauri、GUI、常驻服务或自动实验。
- 电压源 0–14 V，正常步长 ≤0.1 V、间隔 ≥0.050 s，故障与关闭立即写零。
- Gain 0–200 mA、15–40 °C；开启电流前 TEC 开启且温度在目标 ±0.2 °C 的采样证据覆盖至少 5.0 s；先关电流再关 TEC。
- MDT 75 V，正常步长 ≤0.1 V、间隔 ≥0.050 s；Piezo 故障停住保持、绝不自动归零。
- 保留左/右序列号、坐标映射、单台支持、名义位移授权和已有位移限制。
- 不自动重连、不自动恢复输出、不改变 PM400/OSA 面板测量设置。
- 所有故意阻塞的测试都有 Event 释放和 finally 清理；不创建不可回收的测试线程。
- 用户已有 JSON 修改不暂存、不覆盖；Git 作者身份未配置，不猜测或修改用户身份。代码可验证后保留未提交状态。

## 执行准备与工作区

- [x] 已取得隔离工作区同意；在 `.worktrees/driver-hardening`、`codex/driver-hardening` 实施，未修改原 main 的代码与实验配置。
- [x] 核查后没有适用的原生 worktree 工具；已确认 `.worktrees` 被忽略，仅复制本设计和计划。
- [x] 主代理已读取工作区 AGENTS.md、设计、计划及必需技能和测试指引。
- [x] 已记录原目录与隔离目录状态，保留用户修改；未安装或修改环境。
- [x] 隔离目录离线基线：793 tests，81.866 s，OK；使用 VISA、Agg，临时文件定向 `tmp/driver-hardening`。

```powershell
$taskTemp = Join-Path (Get-Location) 'tmp/driver-hardening'
New-Item -ItemType Directory -Path $taskTemp -Force | Out-Null
$env:TEMP = $taskTemp
$env:TMP = $taskTemp
$env:MPLCONFIGDIR = Join-Path $taskTemp 'matplotlib'
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:MPLBACKEND = 'Agg'
conda run -n VISA --no-capture-output python -B -m unittest discover -s Code/Debugs -p 'test*.py' -q
```

若基线失败，先区分环境失败、已有代码失败和测试替身失败，保留完整证据；不把失败算作本轮引入，也不先修改无关文件。

## 任务边界与顺序

| 任务 | 产物 | 依赖 |
|---|---|---|
| 1 | Gain 五秒稳定门限 | 基线 |
| 2 | VISA 租约和资源登记 | 基线 |
| 3 | PM400 使用共享租约 | 2 |
| 4 | OSA 共享租约及有界清理 | 2、3 |
| 5 | 电压源归零证据 | 基线 |
| 6 | 通用会话 | 4、5 |
| 7 | 两个实验迁移 | 1、6 |
| 8 | 文档、环境清单与全量验收 | 1–7 |

每个任务先报告 RED 的准确失败原因，再实现 GREEN。不同任务可以分别审查；同一个文件的修改必须串行交付，禁止多个实现者同时编辑 test_drivers.py。

## Task 1: Gain 稳定时间窗

**Files:** 修改 `Code/Utils/gain.py`、`Code/Debugs/test_drivers.py`。

**Interfaces:** 保留 GainDriver 所有公开签名。内部新增固定常量 `_STABILITY_SECONDS = 5.0` 和 `_STABILITY_SAMPLES = 6`；仍使用现有 epoch、时钟和 watchdog condition。

- [x] **Step 1 — RED:** 在 GainSafetyTests 增加真正调用 enable_current 的回归：使用现有 ready_gain_driver、FakeClock、FakeGainSerial，只提供五个一秒间隔样本，断言拒绝开启且未写 Q=1。

```python
def test_five_samples_covering_four_seconds_cannot_enable(self):
    clock = FakeClock()
    driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
    try:
        fake.queue_temperature(*([22.0] * 5))
        for index in range(5):
            if index:
                clock.advance(1.0)
            driver._watchdog_iteration()
        fake.queue(b'READY;R=1\r\n', b'READY;C=150.000\r\n',
                   b'READY;T=22.000\r\n', b'READY;Q=1\r\n',
                   b'READY;C=3.000\r\n')
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        self.assertNotIn(b'STQA000001\r\n', fake.writes)
    finally:
        fake.replies.clear()
        driver.close()
```

现有 FakeGainSerial.replies 是回复列表；finally 清除未消费的开启回复后，使用其既有 auto_shutdown_replies 生成 Q=0/D=0 清理响应。

- [x] **Step 2:** 执行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_drivers.GainSafetyTests -q`。新增回归应因旧实现错误允许开启而失败，不应因串口替身缺少响应而失败。
- [x] **Step 3 — GREEN:** 所有稳定历史截断改为最后六个样本。稳定谓词同时检查数量、首尾跨度、相邻间隔和新鲜度；保留原有重置条件与 enable_current 临门复查。

```python
times = self._stable_sample_times
if self._stable_samples < _STABILITY_SAMPLES or len(times) < _STABILITY_SAMPLES:
    return False
if times[-1] - times[0] < _STABILITY_SECONDS:
    return False
```

这段检查插入现有谓词，不替代后续 freshness 和间隔检查。同步修改依赖“已稳定”的旧测试准备数据为六点五秒，不更改其真实故障断言。

- [x] **Step 4:** 补六点五秒通过、4.999 s 拒绝、五点即使较稀疏也拒绝、断档重置、目标/TEC 改变重置、过期及并发关闭回归，运行完整 `Code.Debugs.test_drivers`。
- [x] **Step 5:** 审查仅 Gain 与对应测试的差异，记录 RED/GREEN 证据；作者信息已由用户配置时才提交这两个文件。

## Task 2: 共享 VISA 所有权与规范化

**Files:** 新增 `Code/Utils/visa.py`、`Code/Debugs/test_visa.py`。

**Interfaces:**

- `acquire_visa_manager(*, resource_manager: object | None = None, factory: Callable[[], object] = pyvisa.ResourceManager) -> VisaManagerLease`。
- `VisaManagerLease.manager: object`、`released: bool`、`close() -> None`。
- `VisaManagerLease.reserve_resource(name: str) -> VisaResourceReservation`。
- `VisaResourceReservation.canonical_name: str`、`released: bool`、`release() -> None`。
- close 只释放管理器租约；驱动先确认资源及其调用已释放，再 release reservation。存在未释放 reservation 时 lease.close 必须拒绝。
- 所有权元数据包含实际 manager 身份、活跃租约 token、借用标记、closing 状态、关闭失败证据。工厂对象相同不意味着管理器相同，反之亦然。

- [x] **Step 1 — RED:** 测试两次 acquire 返回同一 FakeManager：关闭第一个租约不调用 manager.close，关闭最后一个恰好一次；借用优先、重复 close 幂等、资源占用阻止关闭。

```python
class RecordingManager:
    def __init__(self):
        self.close_calls = 0
    def close(self):
        self.close_calls += 1

def test_last_lease_alone_closes_manager(self):
    manager = RecordingManager()
    first = acquire_visa_manager(factory=lambda: manager)
    second = acquire_visa_manager(factory=lambda: manager)
    try:
        first.close()
        self.assertEqual(manager.close_calls, 0)
        second.close()
        self.assertEqual(manager.close_calls, 1)
        second.close()
        self.assertEqual(manager.close_calls, 1)
    finally:
        first.close()
        second.close()
```

- [x] **Step 2:** 运行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_visa -q`，观察新 API 尚不存在的预期失败，再逐个补行为测试。
- [x] **Step 3 — GREEN:** 实现按 manager 对象身份管理的登记表；把对象实际 close 放全局锁外，只在最后租约登记 closing 后调用。失败保留最后租约，成功才删除活动所有权。

```python
# 最后租约执行外部 I/O 的结构；entry 和 token 均由租约自身保存。
with registry_lock:
    if entry.closing:
        raise InstrumentConnectionError('VISA manager is already closing')
    entry.closing = True
try:
    manager.close()
except BaseException as error:
    with registry_lock:
        entry.close_error = error
        entry.closing = False
    raise
```

只在确认“最后租约、非借用、无资源占用”的分支执行该结构。非最后租约移除自身 token 后返回；借用管理器始终不自动关闭。借用标记不能在一个借用者释放后被仍存活的工厂租约遗忘；弱引用元数据不可反向强持有已结束的借用对象。

资源名称先去外围空白，再由 backend resource_info 的 expanded resource_name 或 PyVISA ResourceName 解析标准名。别名解析失败不得拿原字符串直接打开。不要全串转大写而改变大小写敏感的标识字段。登记覆盖 OSA 和 PM400，而非每个类一个表。

- [x] **Step 4:** 测试 GPIB0::4::INSTR 与 GPIB::4 同一目标、backend 别名展开、USB 有效完整名、非法名拒绝、同目标双占用拒绝、不同目标独立。
- [x] **Step 5:** 用 Event 测试最后租约关闭卡住时新登记拒绝，其他管理器不受阻；关闭失败后显式重试；并发释放只关闭一次；借用/工厂混合顺序。finally 释放所有阻塞点。
- [x] **Step 6:** 使用 `Mock(spec=VisaLibraryBase)` 和真实 `pyvisa.ResourceManager(lib)` 验证对象复用及关闭行为；禁止调用真实 open_visa_library。所有资源都用模拟传输。审查登记表、资源和 helper 不遗失后记录结果。

## Task 3: PM400 迁移到共享租约

**Files:** 修改 `Code/Utils/pm400.py`、`Code/Debugs/test_pm400.py`；复用 Task 2 测试传输，不改测量功能。

**Interfaces:** PM400 公开签名和 facade 不变；内部持有 `_visa_lease`、`_resource_reservation`。保留 `_manager` 为实际 manager 引用，使现有 helper subject 身份判断保持清楚。

- [x] **Step 1 — RED:** 在 test_pm400 增加两个有效 USB 资源共享一个管理器的测试，关闭第一个后第二个仍可查询且未被 close；同时测一个连接失败不影响已有另一台。

```python
first = PM400('USB0::0x1313::0x8078::P1::INSTR',
              resource_manager_factory=lambda: shared_manager)
second = PM400('USB0::0x1313::0x8078::P2::INSTR',
               resource_manager_factory=lambda: shared_manager)
try:
    first.connect()
    second.connect()
    first.close()
    self.assertFalse(second_resource.closed)
    self.assertEqual(second.system.get_scpi_version(), '1999.0')
finally:
    first.close()
    second.close()
```

shared_manager 在测试中按名称返回两个 FakeVisaResource，close 时关闭所有已创建资源，以再现真实管理器语义；两个资源预置现有 *IDN?、传感器 ID 与版本回复。

- [x] **Step 2:** 运行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_pm400 -q`，先看到上述跨资源独立性断言失败。
- [x] **Step 3 — GREEN:** 用公共租约取得 manager 并 reserve canonical resource 后再 open。资源关闭 helper 全部回收成功后 release reservation，再通过既有有界 helper 调用 lease.close，而不是直接 manager.close。

```python
self._visa_lease = acquire_visa_manager(
    resource_manager=self._injected_manager,
    factory=self._resource_manager_factory,
)
self._manager = self._visa_lease.manager
self._resource_reservation = self._visa_lease.reserve_resource(self.resource_name)
self._resource = self._manager.open_resource(
    self._resource_reservation.canonical_name
)
```

connect 发生异常时保留已取得的 lease，交给同一清理流程。借用路径也必须释放 lease，只是不关闭底层 manager。清理 helper 的晚完成处理必须同时清理 lease 引用，不能只清 `_manager`；未解决 sibling helper 时不得释放 reservation。

- [x] **Step 4:** 将旧测试里用于资源命名的无效 USB 简写更新为有效 VISA 名，保留原故障场景，不给生产 parser 增加“测试特例”。
- [x] **Step 5:** 回归 PM400 所有 capability gate、参数回读、超时恢复、cancel、helper 晚完成、重复会话及 front-panel 无写入测试；再运行 Task 2 测试并核查无租约残留。

## Task 4: OSA 生命周期与共享资源

**Files:** 修改 `Code/Utils/osa.py`、`Code/Debugs/test_drivers.py`；新增 `Code/Debugs/test_osa_lifecycle.py` 以隔离并发回归。

**Interfaces:** 保留 acquire/connect/close 和所有旧位置参数；构造函数末尾新增 keyword-only `resource_manager=None`、`close_timeout=2.0`。公开 `cleanup_error`；使用 Task 2 租约和 reservation。

内部边界：生命周期锁、单个采集事务锁、取消 generation、可追踪清理 helper；每个 helper 带 kind、subject、done Event、thread、error。close 使用一个真实 monotonic 总 deadline，各次等待传剩余预算。

- [x] **Step 1 — RED:** 新增真实 OSA + PM400、真实 PyVISA manager + 模拟 backend 的跨设备测试，复现关闭 OSA 关闭了 PM；另加两个底层 close 均失败的回归。

```python
resource.close.side_effect = OSError('resource close failed')
manager.close.side_effect = OSError('manager close failed')
driver.connect()
try:
    with self.assertRaises(DriverError):
        driver.close()
    self.assertEqual(driver.state, DriverState.FAULT)
    self.assertIsNotNone(driver.cleanup_error)
    self.assertIs(driver._resource, resource)
finally:
    resource.close.side_effect = None
    manager.close.side_effect = None
    driver.close()
```

测试 resource/manager 采用现有连接替身与正确 *IDN?；不使用真实硬件工厂。

- [x] **Step 2:** 执行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_osa_lifecycle Code.Debugs.test_drivers.OsaConnectionTests Code.Debugs.test_drivers.OsaAcquisitionTests -q`，记录旧实现假报 DISCONNECTED 和连带关闭的失败。
- [x] **Step 3 — GREEN:** 先接入租约，再让所有生命周期状态变更在锁内发布。close 先增取消 generation，阻止新事务；acquire 每次发起 sweep、重试和最终发布前验证捕获的 generation。

```python
with self._lifecycle_lock:
    if self._closing or generation != self._cancel_generation:
        raise InstrumentConnectionError('OSA acquisition was cancelled')
    self.state = DriverState.READY
    return spectrum
```

最终检查与成功发布不可分开加锁。串口/VISA I/O 不持生命周期锁。采集中任何 BaseException 进入故障清理，保留原异常，不能被 helper 错误覆盖。

- [x] **Step 4:** 实现按剩余预算等待/回收的 abort、resource-close、lease-release helper。预算耗尽进入 FAULT 并保存对象；重复 close 回收原 helper，不启动同对象同动作的并行 helper。不允许未结束 acquire/abort 持有资源时发布完全释放。
- [x] **Step 5:** 根据新的清理契约更新旧测试：清理失败现在应抛 typed error、保留 FAULT 和句柄；不要保留旧的“吞错成功”断言。原操作异常仍按原类型/对象保真。
- [x] **Step 6:** Event 驱动回归：采集阻塞时 close 在 2.0 s 预算附近返回故障、晚成功不得发布谱线、资源 close 卡住、显式二次清理、SystemExit、KeyboardInterrupt、并行 connect/close、同资源重复构造、不同资源独立；无面板设置写入。
- [x] **Step 7:** 执行 Task 2、PM400、OSA 和 test_drivers 回归；核查每个 Event 最终释放、每个 helper 最终 join，报告时间预算允许的测试调度余量。

## Task 5: 电压源归零证据

**Files:** 修改 `Code/Utils/voltage.py`、`Code/Utils/__init__.py`、`Code/Debugs/test_drivers.py`；新增 `Code/Debugs/test_voltage_evidence.py`。

**Interfaces:** 保留现有方法返回与参数；新增 `VoltageSource.zero_evidence: ZeroEvidence` 只读属性。类型定义如下，枚举的 value 用于日志/可序列化摘要。

```python
class ZeroState(Enum):
    UNKNOWN = 'unknown'
    COMMAND_SENT = 'command_sent'
    MEASURED_ZERO = 'measured_zero'

@dataclass(frozen=True)
class ZeroEvidence:
    state: ZeroState
    sent_at: float | None
    observed_at: float | None
    voltage_v: tuple[float, ...] | None
```

额外内部字段：零命令 generation、开始读取遥测时捕获的 generation、成功发送后的 status sequence 下界。固件没有命令序号或测量时间戳，文档只能称“本次写入后上位机观察到的全零遥测”，不能宣称独立固件 ACK 或精确物理同步。

- [x] **Step 1 — RED:** 使用可注入的现有电压源串口替身：完整接收零帧但不提供新遥测，断言 zero_evidence 不是 MEASURED_ZERO；close 必须在确认预算耗尽后仍关闭串口并报告 unknown。

```python
source.zero(emergency=True)
self.assertEqual(source.zero_evidence.state, ZeroState.COMMAND_SENT)
self.assertIsNone(source.zero_evidence.observed_at)
with self.assertRaises(DriverError):
    source.close()
self.assertFalse(serial_port.is_open)
self.assertEqual(source.zero_evidence.state, ZeroState.UNKNOWN)
```

测试必须先用有效遥测完成 connect，然后禁用替身后续遥测；不是把启动失败当作关闭失败。

- [x] **Step 2:** 运行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_voltage_evidence -q`，观察新证据接口/行为未实现的失败。
- [x] **Step 3 — GREEN:** 每次 write 尝试前使旧确认失效；完整写入全零帧后发布 COMMAND_SENT。reader 在新 generation 中开始读取、收到比基线更新且未过期的全零帧后发布 MEASURED_ZERO；八通道均须满足 `abs(v) < 0.05`。

```python
if (read_generation == self._zero_generation
        and read_started_at >= sent_at
        and sequence > minimum_sequence
        and all(abs(value) < 0.05 for value in status.voltage_v)):
    evidence = ZeroEvidence(ZeroState.MEASURED_ZERO, sent_at,
                            status.received_at, status.voltage_v)
```

所有字段在现有 status condition 下读取/发布。新非零帧、写失败、遥测故障或证据过期均使确认失效；不能让迟到 reader 覆盖更新的命令证据。

- [x] **Step 4:** 修改 reader 的 CLOSING 分支，使它只继续获取关闭确认所需的遥测，不继续普通控制或重试引起新输出。close 写零后最多等 1.0 s，然后设置 stop、关闭串口和 join；reader 错误可提前结束等待。
- [x] **Step 5:** 未确认时保留输出 unknown 的错误，但释放已经关闭的串口/已退出线程引用；资源未确认关闭时保留对象。关断未知与资源未释放分别记录，不再把完整 write 当成物理确认。
- [x] **Step 6:** 覆盖旧零帧、跨越 write 的在途读取、命令后非零、八通道中一个不为零、恰好容差边界、写重试、延迟全零、关闭中断、归零后非零命令、通信丢失和 immutable evidence。
- [x] **Step 7:** 修正旧替身，使成功关闭测试真实提供关闭之后的遥测；不能给驱动加关闭确认开关来跳过验证。运行全部电压测试与 test_drivers，检查紧急 zero 不等待遥测。

## Task 6: 通用会话与清理报告

**Files:** 新增 `Code/Setups/session.py`、`Code/Debugs/test_session.py`，修改 `Code/Setups/__init__.py`。

**Interfaces:**

- `InstrumentSession(*, osa=None, voltage=None, gain=None, pm400=None, fiber=None, log=None)`：交付对象的生命周期由会话协调；构造不做 I/O。
- `connect() -> InstrumentSession`、`close() -> CleanupReport`、`request_stop() -> None`、`check_health() -> tuple[DeviceHealth, ...]`。
- `safe_shutdown(voltage, gain, osa, log, *, pm400=None, fiber=None) -> None`：兼容旧四位置参数调用，内部使用会话统一清理。
- `CleanupStep` 为 frozen 数据类，字段 `role: str`、`action: str`、`error: BaseException | None`。
- `CleanupReport` 为 frozen 数据类，字段 `steps: tuple[CleanupStep, ...]`、`voltage_zero: ZeroEvidence | None`、`unreleased: tuple[tuple[str, object], ...]`；`ok` 为只读属性。
- `DeviceHealth` 为 frozen 数据类，字段 `role: str`、`state: DriverState | None`、`cached: bool`、`observed_at: float | None`。未知或不支持的 state 不伪装为 READY。
- 清理异常附带 `cleanup_report`。若已有操作异常，则原异常作为主异常传播，报告附加其上；记录失败不能中断后续关断。

FiberCouplingSetup.connect 是类工厂，不是实例重连方法。`fiber` 仅接受调用方已单独连接并授权交付的 setup；会话 connect 不调用其 connect，不枚举设备。缺失一侧合法；基线尚未采用也不是连接故障。会话不读取其私有 MDT 或重新建立运动权限。

- [x] **Step 1 — RED:** 以仅记录公开方法的测试替身验证副作用边界、严格关断顺序及 Piezo 只有 close。模拟每个步骤失败，并证明后续动作仍发生。

```python
events = []
class Piezo:
    def close(self):
        events.append('fiber.close')
class Gain:
    def disable_current(self):
        events.append('gain.current_off')
        raise OSError('current confirmation failed')
    def disable_tec(self):
        events.append('gain.tec_off')
    def close(self):
        events.append('gain.close')

session = InstrumentSession(gain=Gain(), fiber=Piezo())
self.assertEqual(events, [])
with self.assertRaises(OSError) as raised:
    session.close()
self.assertEqual(events, ['gain.current_off', 'gain.tec_off',
                          'gain.close', 'fiber.close'])
self.assertIsNotNone(raised.exception.cleanup_report)
```

- [x] **Step 2:** 执行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_session -q`，记录新接口缺失/清理顺序的预期失败。
- [x] **Step 3 — GREEN:** 连接顺序维持 OSA → Voltage → Gain，再处理显式交付的 PM400；fiber 不重连。出错进入统一清理。cleanup 动作固定为 zero → current_off → tec_off → gain.close → voltage.close → osa.close → pm400.close → fiber.close，只处理非 None 角色。

```python
steps = []
for role, action_name, action in actions:
    error = None
    try:
        action()
    except BaseException as caught:
        error = caught
    steps.append(CleanupStep(role, action_name, error))
```

actions 由上述固定公开调用生成，不接受实验注入任意原始串口命令。先完成安全动作和记录，再 best-effort 日志；将首次清理错误按兼容顺序传播并附加完整 report。

- [x] **Step 4:** 实现同一 session 的幂等 close：已确认关闭角色不重连/重做动作，未解决角色保留引用用于显式再清理；先前错误报告不可被成功重试修改。close/request_stop 相互同步但不把锁跨越无界 I/O。
- [x] **Step 5:** check_health 只检查公开状态和缓存元数据，不启动传感器查询；请求停止或驱动 FAULT/CLOSING/意外 DISCONNECTED 则抛 DeviceFault/InstrumentSafetyError。正常 READY 与 ACTIVE 可通过；未知状态报告/拒绝规则一致，不能因对象缺少 state 就默认安全。
- [x] **Step 6:** context manager 保留 KeyboardInterrupt/SystemExit/操作原异常；连接未完成、所有可选组合、损坏日志、重复关闭、多个清理失败、未知电压、stop 无 I/O、Fiber 无任何归零方法调用均有测试。

## Task 7: 两个实验迁移协调逻辑

**Files:** 修改 `Code/Experiments/sil_hysteresis/experiment.py`、`Code/Experiments/sil_repeat/experiment.py`、`Code/Experiments/sil_hysteresis/simulate.py`；按实际复用关系检查 `Code/Experiments/sil_repeat/simulate.py`。修改对应 experiment/CLI/e2e 测试，不改任何 JSON、storage、analysis、figure 或 scan 算法。

**Interfaces:** 消费 Task 6 InstrumentSession、safe_shutdown；实验入口参数和 RunSummary/RepeatRunSummary 不变。保留 `sil_hysteresis.experiment.safe_shutdown` 的兼容导出。

- [x] **Step 1 — RED:** 在已有 Fake 仪器上增加可控公开 state。让 Gain 在一次 OSA 采集返回时进入 FAULT；断言当前结果不被作为正常点保存、下一次电压命令不发生、统一清理全部执行。

以下测试放入已有 RepeatExperimentTests，复用 config_for、fake_bundle、incrementing_clock；增加 DriverState、DeviceFault 导入。相同情形在 hysteresis 的现有 runner fixture 中补一项。

```python
def test_gain_fault_after_acquisition_stops_before_next_voltage(self):
    with tempfile.TemporaryDirectory() as root:
        config = config_for(Path(root), cycles=1, points=2)
        store = RepeatRunStore.create(config, run_mode='similar')
        events = []
        bundle = fake_bundle(events)
        original_acquire = bundle.osa.acquire
        def acquire_then_fault(*args, **kwargs):
            spectrum = original_acquire(*args, **kwargs)
            bundle.gain.state = DriverState.FAULT
            return spectrum
        bundle.osa.acquire = acquire_then_fault
        runner = RepeatExperimentRunner(
            config, store, lambda: bundle,
            postprocessor=InlinePostprocessor(lambda request: None),
            coupling_gate=lambda point: None,
            sleep=lambda seconds: None, clock=incrementing_clock(),
        )
        with self.assertRaises(DeviceFault):
            runner.run()
        first_acquire = events.index('osa.acquire.1')
        self.assertFalse(any(event.startswith('voltage.set.')
                             for event in events[first_acquire + 1:]))
        self.assertEqual(events[-6:], [
            'voltage.zero.emergency', 'gain.disable_current',
            'gain.disable_tec', 'gain.close', 'voltage.close', 'osa.close',
        ])
```

- [x] **Step 2:** 运行 `conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_sil_experiment_cli Code.Debugs.test_sil_repeat_experiment -q`，观察旧 runner 不在采集后检查跨仪器 FAULT 的失败。
- [x] **Step 3 — GREEN:** 两个实验直接从公共 session 模块导入。保留现有 bundle 工厂、驱动连接顺序及 finally 位置；建立一个持有同一批对象的 session，连接后、每次输出修改前、采集返回后调用 check_health。

```python
from Code.Setups.session import InstrumentSession, safe_shutdown

session = InstrumentSession(osa=bundle.osa, voltage=bundle.voltage,
                            gain=bundle.gain, log=self.log)
session.connect()
session.check_health()
```

runner 保留 session 到 finally 清理完毕；报告有未释放对象时不能无条件丢弃所有 session 引用。主异常、后处理 drain 的既有优先级不变。

- [x] **Step 4:** 在模拟器真实公开契约中加入 DriverState：构造 DISCONNECTED、connect READY、输出/采集时匹配真实角色状态、close DISCONNECTED；不改随机数、谱线数据、时间模型和实验计算。需要控制操作的模拟测试先显式 connect，纯 spectrum_for 计算仍可独立使用。更新测试替身，不在真实 session 增加识别“Fake”类名的分支。
- [x] **Step 5:** 验证旧 safe_shutdown 导入仍可用，sil_repeat 不再从另一个实验导入安全协调。保留其原本复用的分析/扫描工具，本轮不重构那些依赖。
- [x] **Step 6:** 运行两套 SIL config/CLI/storage/experiment/e2e/simulation 测试；对同一 seed 比较迁移前后模拟谱线和扫描点序列一致。验证 resume 已完成记录不触碰硬件、不重复扫描。

## Task 8: 文档、环境与整体验收

**Files:** 新增 `README.md`、`environment.yml`、`docs/acceptance.md`、`Code/Debugs/test_environment_docs.py`；修改 AGENTS.md 只记录已经通过验证的新边界；更新本计划各任务勾选与证据。

**Interfaces:** 不增加硬件入口；不应用环境清单。验收表区分“离线验证通过”“历史实机证据”“本版本实机未验证”。

- [x] **Step 1:** 先增加文档契约测试并观察 README/environment.yml 尚不存在时失败。只读收集实际直接依赖和版本，核对源码 import；不要把当前环境所有包机械导出。VISA 名称保持不变。

```python
def test_environment_keeps_visa_name_without_machine_prefix(self):
    root = Path(__file__).resolve().parents[2]
    content = (root / 'environment.yml').read_text(encoding='utf-8')
    self.assertIn('name: VISA', content.splitlines())
    self.assertFalse(any(line.strip().startswith('prefix:')
                         for line in content.splitlines()))
    self.assertIn('PyVISA==1.14.1', content)
```

同一文件检查 README 中使用 `conda run -n VISA`、验收表明确 `未验证`，以及配置仍保留真实左右序列号。所有检查只读取文件，不执行文档里的硬件示例。

```powershell
conda run -n VISA --no-capture-output python -B -c "import sys, importlib.metadata as m; print(sys.version); print({p:m.version(p) for p in ['numpy','pyserial','PyVISA','PyMeasure','matplotlib','Pillow']})"
```

2026-09-04 已读版本为 Python 3.10.16、NumPy 2.2.4、pyserial 3.5、PyVISA 1.14.1、PyMeasure 0.15.0、Matplotlib 3.10.1、Pillow 11.1.0。当前源码不直接导入 SciPy/Pandas，不把它们加入项目直接依赖。现有测试直接导入 PIL.Image，Pillow 记录为测试所需依赖。

- [x] **Step 2:** environment.yml 使用下列直接依赖及测试依赖清单，不包含 prefix、个人路径、凭据或未使用的大型 GUI 依赖。

```yaml
name: VISA
dependencies:
  - python=3.10.16
  - pip
  - pip:
      - numpy==2.2.4
      - pyserial==3.5
      - PyVISA==1.14.1
      - PyMeasure==0.15.0
      - matplotlib==3.10.1
      - Pillow==11.1.0
```

明确系统 VISA backend/USB 驱动不由该 YAML 安装，清单尚未在新机器重建验证。读取 backend 软件证据不等于允许列出或打开设备。

- [x] **Step 3:** README 说明最小调用、共享 manager 所有权、用后关闭、清理错误处理、模拟与硬件命令区别、不能保证硬杀进程后的关断。示例硬件代码不作为文档测试自动执行。
- [x] **Step 4:** acceptance.md 对 OSA、Voltage、Gain、PM400、两台 MDT/Fiber 分项记录测试覆盖、序列号/探头/固件证据来源和未验证项。保留 MAX312D 名义系数，标明没有位移实测、回差或闭环精度保证。
- [x] **Step 5:** 以 unittest 读取 YAML/文档检查安全约束、默认资源和模拟指令一致，不连接设备；用 `python -B` + `ast.parse` 进行语法检查而不是在 Utils 生成 pycache。
- [x] **Step 6:** 完整离线测试与资源审计：

```powershell
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:MPLBACKEND = 'Agg'
conda run -n VISA --no-capture-output python -B -m unittest discover -s Code/Debugs -p 'test*.py' -q
conda run -n VISA --no-capture-output python -B -m Code.Debugs.check_osa --help
conda run -n VISA --no-capture-output python -B -m Code.Debugs.check_pm400 --help
conda run -n VISA --no-capture-output python -B -m Code.Debugs.check_fiber_stages --help
git diff --check
```

- [x] **Step 7:** 在同一测试进程统计 VISA 活跃租约/reservation、OSA helper、PM400 helper、Voltage reader、Gain watchdog；全套测试最终无活动资源或线程。审计只能使用离线测试工具；不要向生产驱动加入“关闭所有真实设备”的捷径。
- [x] **Step 8:** 根据 verification-before-completion 技能读取最新完整结果，报告实际用例数量、耗时及失败；requesting-code-review 审查共享所有权、晚完成、关断顺序及实验兼容性。修复审查发现后只重跑相关测试及最终全量。
- [x] **Step 9:** 用 finishing-a-development-branch 技能交付。仅提交/合入本轮文件，保留用户现有 JSON 修改。没有身份或合入授权时，交付验证通过的代码和差异，不擅自设置 Git 用户、删除工作区或把未合入说成已生效。

## 计划自查与交付记录

设计映射：§4 → Task 1；§5 → Tasks 2–4；§6 → Task 4；§7 → Task 5；§8 → Tasks 6–7；§9 → Task 8；§10–11 → 准备、各任务 RED/GREEN 与最终验收。

本计划写成时尚未实施。658 项通过是前一轮审查的底层证据，不是修改后的验收结果。每项任务完成时在本文件记录新测试命令、结果和审查结论；未执行的实机验证继续明确标为未验证。

### 执行进度（2026-09-04）

- 隔离工作区基线：完整离线 793 tests，81.866 s，OK。
- Task 1 已完成且审查通过：`conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_drivers -q`，189 tests，7.757 s；边界测试修正后 Gain 定向回归 34 tests，2.082 s，OK。RED 证明旧版五点四秒允许开启；新增六点 4.999 s 边界测试并保留原安全机制。
- Task 2 已完成且经两轮修正审查通过：`conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_visa -q`，26 tests，0.021 s，OK。覆盖登记/最后关闭竞态、对象析构重入、外部借入所有权、资源规范化与真实 PyVISA 模拟后端。
- Task 3 已完成且审查通过：`conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_pm400 Code.Debugs.test_visa -q`，142 tests，1.854 s，OK；同进程审计 142 tests，1.791 s，五个共享 VISA 登记表残留均为 0。
- Task 4 已完成且审查通过：VISA/PM400/OSA/test_drivers 合计 348 tests，12.962 s，OK；同进程复跑 348 tests，12.870 s，VISA 登记表均为 0、OSA helper 无残留；最后定向 OSA 62 tests，3.393 s，OK。
- Task 5 已完成且经一轮修正审查通过：组合回归 366 tests，22.870 s；修复迟到错误覆盖退出证据后，电压证据 19 tests，4.882 s 与 test_drivers 组合 208 tests，18.200 s，OK。
- Task 6 已完成且经一轮修正审查通过：组合回归 753 tests，26.967 s；修复连接/关闭共用清理记录后，session 27 tests，0.016 s，OK。
- Task 7 已完成且经一轮修正审查通过：完整两套 SIL 回归 142 tests，67.744 s；修复模拟器分步关断后，71 tests，27.849 s，OK。40 组谱线与 5 组扫描序列指纹不变。
- Task 8 Steps 1–7 已完成：环境契约 3 tests/0.460 s；最终同进程离线审计 902 tests/100.107 s，五个 VISA 登记表均为 0、命名仪器线程为空；独立 discovery 902 tests/99.677 s，退出 0。AST 117 个 Python 文件及 OSA/PM400/Fiber `--help` 均退出 0，`git diff --check` 退出 0。文档经显式审阅，未以偶然措辞断言代替验收。未触及实机、环境安装、原目录用户配置或结果。Step 8 的 controller 独立验证/审查与 Step 9 的分支交付仍显式待办。
- 逐批完整证据和实施裁定保留在隔离工作区 `.superpowers/sdd/2026-09-04-driver-hardening/`；Git 作者身份未配置，代码暂不提交或合入。

### 实施预检补充接口

历史状态补记（本次批准继续之前）：一次完整总审及一次合并修复波次/限定复审均已完成，修正后覆盖回归360项、定向资源审计44项通过。controller 全量911项/98.213s出现2失败1错误，五个VISA登记表为0但一个模拟MDT监控线程残留。根因为当时未修改的close_gated_driver测试辅助函数放行时钟早于关闭意图的竞态，已用受控调度离线复现并清理。彼时Task8 Step8整体验收及Step9交付未完成，未提交/合入，也未把之前902项通过当作最终通过。

最终状态补记（2026-09-05）：用户明确批准继续后，仅修复MDT离线夹具同步及新测试失败清理，生产MDT未改动。确定性RED后修复，MDT240项/4.044s及强制初始化失败清理验证通过；两轮限定复审后全部问题解决。controller在最终稳定文件上重新执行同进程全量913项/97.868s，0失败/0错误/0跳过、五个VISA登记表均为0且全部新增存活线程为空；随后标准discovery913项/98.055s，OK/退出0。Code60文件AST、三个诊断--help和diff-check通过。其余69个受保护文件哈希不变。本计划8个任务及Task8 Steps8–9的离线验证/未提交交付完成；所有实机和Git整合操作仍未执行，隔离工作区和唯一证据副本保留。精确证据见docs/acceptance.md和本计划mdt-final-verification.md。

本地合入补记（2026-09-05，后于上述离线交付）：用户选择本地合入main并提供Qian <q.zhang1@wustl.edu>身份。仅以命令级Git参数提交29个已审查文件为f61597da49d46e8d486a1fa256650dca6af5dcc4，并快进合入main；未修改全局或仓库身份设置，未推送。提交前全量913项/98.345s、合入后原工作目录全量913项/98.047s均通过，全部新增存活线程和五个VISA登记表均为空。原有7份实验配置哈希及未提交状态不变；重叠文档原内容/暂存状态已保存在本地stash和tmp/merge-driver-hardening-20260905。全部审查与测试证据已复制回原工作目录并校验。当前整合证据见本计划local-merge.md；前面的未提交状态仅描述当时交付，不代表当前main状态。实机仍未验收。

- Task 5 增加只读 `VoltageSource.resources_released`，仅报告串口/reader 是否均已确认释放；与 `zero_evidence` 独立，绝不代表实测归零。用于会话层无需私有字段即可区分未知输出和未释放资源。
- Task 6 允许在 `FiberCouplingSetup` 内增加只读、无 I/O 的聚合缓存 `state`。会话层仅消费公开状态，不窥探私有 MDT；单侧、未采用基线和正常的操作者基线证明不能被误判为连接故障。
- 显式借入管理器需支持弱引用；不支持时在接受租约前拒绝且不关闭外部对象，避免永久强引用登记。
- Task 8 不用逐字匹配文档措辞的测试代替验证；执行有意义的环境清单检查、文档审查与完整运行验收。以上裁定及代价在执行 ledger 保留。
