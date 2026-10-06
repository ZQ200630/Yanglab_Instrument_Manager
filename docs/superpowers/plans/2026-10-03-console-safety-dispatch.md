# Console Safety Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让长时间仪器操作不再阻塞其它设备的安全关断，并真实显示命令结果、资源责任和 Gain 各字段的新鲜度。

**Architecture:** 一个 Anaconda VISA Python 进程持有全部设备，固定的设备级执行槽隔离普通操作、观察和白名单安全动作。Rust 使用有界优先写入器和按请求 ID 分发的回复 broker，前端使用设备级 busy 与身份/代次检查。关断完成必须经过旧调用退出及最终安全调用屏障。

**Tech Stack:** 现有 Python 标准库 threading/concurrent.futures/unittest、现有 Code 公共驱动、Rust std + serde/serde_json + Tauri 2、原生 ES modules + Node test runner；不新增依赖。

**Spec:** `docs/superpowers/specs/2026-10-03-console-safety-dispatch-design.md`（用户已在本对话回复“确认。”批准；基线提交 `0c9e804`）。

## Global Constraints

- 本次只重构 `App/` 的传输、调度、状态和对应界面。`Code/Utils`、`Code/Setups`、实验代码和参考例程不在修改范围内。
- 不强杀硬件工作进程、不自动重启或重连、不自动重放写操作、不直接发送串口/SCPI 数据。
- Voltage：0–14 V，正常步长不超过 0.1 V、间隔不少于 50 ms；紧急归零使用 `zero(emergency=True)`。
- Gain：0–200 mA、目标 15–40 °C；TEC 开启且温度连续五秒在目标 ±0.2 °C 内，才可由驱动允许开电流。完整关断先电流、后 TEC。
- Fiber 两侧在本次实现中仍共享一个 setup 调度域。允许只连接一侧；不额外引入左右独立运动并发。
- 设置文件版本不因传输升级而改变。
- 活动普通请求上限 32；每个 role 保留一个安全操作槽，另保留一个全局关闭槽。
- 每个 role 最多一个活动普通任务和一个待执行普通任务；更多请求立即拒绝。
- 界面字段年龄超过 5 s 时标 stale；未成功读取过则 unknown。
- 连接后和普通操作完成后安排一次五字段刷新；平时每 2.5 s 安排一次，积压合并而不追赶。
- 所有 Python 使用 Anaconda `VISA`；本规范不授权实机枚举、连接或输出。
- 没有额外批准，不合入 main、不推送、不安装新依赖、不触碰真实仪器输出。
- OSA/PM400 面板设置、PM400 能力门控、Fiber 序列号/坐标/位移/基线规则保持；不增加三维功能。
- 以上软件优先级不保证硬实时响应；命令成功、资源释放、主机零证据和物理测量必须分别报告。

## Review Focus

1. 同一 COM/VISA 名称断开后再次连接，旧页面或旧请求仍携带旧身份：必须拒绝，不以资源名相同恢复权限。测试归属 Task 2、Task 8。
2. 当前安全意图是关电流，又收到关 TEC 或全局关闭：升级而不是吞掉更强意图，不能被等待稳定任务占住安全槽。测试归属 Task 4、Task 6。
3. 普通请求在进入驱动前一瞬间被暂停，安全动作先返回：不能提前宣布最终关断；旧任务退出后必须最后执行安全动作。测试归属 Task 4。
4. 子进程回复已经在途而 UI 卡顿，或旧观察在写入后返回：年龄必须保守，旧值不能恢复 fresh。测试归属 Task 5、Task 8。
5. 清理回复失败但子进程仍活着，用户重试；或清理成功但进程尚未退出：前者创建新尝试，后者只核查退出，均保留原始证据。测试归属 Task 4、Task 6、Task 9。

---

## 工作目录、命令与阶段边界

所有源码工作在已有隔离工作树 `C:/Users/PIC_YangLab/.codex/worktrees/tauri-console/SIL_Experiments`，不要新建另一个工作树，不修改主工作目录中的实验配置。执行前读取该工作树 `AGENTS.md` 和完整 Spec，检查 `git status --short`；发现重叠用户改动先停下。

PowerShell 的测试命令均在隔离工作树根目录运行，Rust 构建另注明目录。定义只用于本终端的变量：

```powershell
$visaPython = 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe'
$nodeExe = 'C:/Users/PIC_YangLab/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node.exe'
& $visaPython -B -m unittest discover -s App/tests -p 'test_*.py' -q
& $nodeExe --test App/tests/*.test.mjs
```

执行前先运行上述基线，不根据历史 69/68 等计数声称当前通过。每个任务先观察目标测试失败，再实现，再运行目标测试和受影响已有测试。测试必须使用 fake 或 `--simulate`；`simulate=False` 的边界测试必须同时注入所有相关 factory、枚举器和资源管理器，不能漏一个真实 I/O 默认值。

本计划中的代码块是具体接口、回归种子和关键实现规则，不是让实现者把未完成 skeleton 提交进产品。每个任务的其余分支由列明的测试矩阵钉住。新增接口只在计划所定义模块内使用；不修改公共仪器驱动接口。

### 文件责任图

| 文件 | 操作及责任 |
| --- | --- |
| `App/worker/contracts.py` | 新建：v2 上下文、请求/结果类型、严格 codec、安全意图分类 |
| `App/worker/scheduler.py` | 新建：role 状态、固定普通/观察执行槽、有界队列、缓存状态 |
| `App/worker/safety.py` | 新建：最终关断屏障、意图升级、同 role 清理协调 |
| `App/worker/observations.py` | 新建：逐字段证据、作废和年龄计算 |
| `App/worker/controller.py` | 修改：短时资源注册表与现有公共 API 适配，不再整段持全局锁 |
| `App/worker/main.py`, `protocol.py` | 修改：非阻塞 ingress、单回复写入器、v2 唯一入口 |
| `App/worker/simulation.py` | 修改：与真实 Gain 公共 API/缓存语义一致 |
| `App/src-tauri/src/reply_broker.rs`, `request_writer.rs` | 新建：按 ID 收包、优先有界发包；两者不拥有设备逻辑 |
| `App/src-tauri/src/worker.rs`, `startup_handshake.rs`, `main.rs` | 修改：短时宿主所有权、进程生命周期、v2 握手和模块注册 |
| `App/web/control-state.js` | 新建：身份、修订号、设备 busy、安全锁存、字段年龄纯函数 |
| `App/web/api.js`, `main.js`, `panels.js`, `view-model.js`, `operations.js`, `lifecycle.js` | 修改：v2 请求、按 role 控件及关闭证据；需要时只微调 `style.css` |
| `App/tests/test_dispatch_contracts.py`, `test_scheduler.py`, `test_safety_dispatch.py`, `test_observations.py`, `test_dispatch_e2e.py` | 新建：下面逐项定义的离线回归 |
| `App/tests/control-state.test.mjs` | 新建：纯前端状态和新鲜度回归 |
| 现有 `App/tests/test_worker_*.py`, `host.test.mjs`, `main.test.mjs`, `ui.test.mjs` | 修改：保留旧安全断言并迁移 v2，不删失败测试换通过 |
| `App/tests/native_worker_fixture.py`, `native_close_fixture.py`, `test_native_close_fixture.py` | 修改：标准库管道夹具 v2 与关闭失败/重试 |
| `App/tests/preview_server.py`, `preview-bridge.js`, `test_preview_server.py`, `test_package_layout.py` | 修改：模拟桥及打包副本使用同一 v2/调度语义 |
| `App/README.md`, `docs/acceptance.md` | 修改：新控制语义、验收事实与仍未验证事项 |

### 公共接口清单（后续任务使用相同名字）

`contracts.py` 定义冻结 dataclass，参数字典在接收边界深拷贝，发布时再次拷贝，禁止共享可变缓存：

```python
from dataclasses import dataclass
from typing import Any, Literal

Role = Literal['osa', 'voltage', 'gain', 'pm400', 'fiber']
Phase = Literal['rejected_before_call', 'superseded_before_call', 'completed',
                'completed_readback_failed', 'failed_after_call_started']

@dataclass(frozen=True)
class Context:
    session_id: str
    connection_id: str | None
    epoch: int

@dataclass(frozen=True)
class Request:
    id: str
    method: str
    params: dict[str, Any]
    context: Context | None

@dataclass(frozen=True)
class Outcome:
    phase: Phase
    context: Context | None
    result: Any = None
    error: dict[str, str] | None = None

@dataclass(frozen=True)
class Observation:
    status: dict[str, Any]
    more: bool = False
```

- `parse_v2(line: str) -> Request`、`encode_v2(request_id: str, outcome: Outcome) -> str`。
- `classify(request: Request) -> str` 返回 `query`、`normal`、`zero`、`current_off`、`tec_off`、`disconnect`、`shutdown` 或 `resume`；不是客户端字段。
- `Scheduler(execute, observe, *, session_id: str, clock=time.monotonic)`：`execute(Request) -> dict` 是 Task 3 的适配器；`observe(role: str, context: Context) -> Observation` 是该 role 的一次公共观察。Gain 每次最多读一个字段，more=true 表示本轮尚有后续字段；调度器在再次调用前重查 stop/普通任务，不一次调用包住五次 I/O。
- `Scheduler.submit(request: Request) -> concurrent.futures.Future[Outcome]`；接收/拒绝均快速返回 Future，绝不在调用者线程执行设备 I/O。
- `Scheduler.context(role: str) -> Context`、`Scheduler.status() -> dict` 仅读缓存；`Scheduler.join(timeout: float) -> bool` 只报告固定线程是否退出，不取消/强杀。
- `ConsoleController.submit(request: Request) -> Future[Outcome]`、`context(role: str) -> Context`、`cached_status() -> dict` 委托 Scheduler。现有 `handle(method, params)` 保留为本地同步兼容包装，只能通过 submit 路径；主 ingress、Rust 和预览桥不能用它串行执行。
- `ConsoleController.close() -> dict` 委托同一个全局清理协调器；不得再创建另一套竞争的清理流程。
- `EvidenceStore(connection_id: str, *, clock=time.monotonic)`：`record(name, value, started_at, revision)`、`invalidate(names, reason)`、`fail(name, message)`、`snapshot() -> dict`，详细语义在 Task 5。

v2 线协议保持一行一个请求、一请求一个终态回复：

```json
{"v":2,"id":"ui-example-1","method":"action","params":{"role":"voltage","name":"zero"},"context":{"session_id":"s1","connection_id":"c1","epoch":3}}
{"v":2,"id":"ui-example-1","ok":true,"phase":"completed","context":{"session_id":"s1","connection_id":"c1","epoch":4},"result":{"effective_intent":"zero"}}
```

严格顶层字段；失败使用 `error:{type,message}` 而不是 `result`。只读 bootstrap `ping/status` 允许 context=null；返回必须包含当前 session_id。其它全局请求使用当前 session_id、connection_id=null、epoch=0；role 请求使用当前 role context。`resume` 是新 method，params 精确为 `{role,confirm:true}`，只改 App 调度资格。

Task 1–6 的新模块先独立测试，不向旧 wire 入口开放不完整并发；Task 7 才一起切换 v2，Task 8 完成 UI 门控。Task 2 提供空闲后关闭线程的基础 shutdown 路径，用于纯 callback 测试的 finally；Task 3 在本地兼容包装中继续沿用既有安全 API 顺序。Task 4 再替换为完整的优先/屏障关闭，不能让前三个任务依赖尚不存在的清理实现来退出测试。期间不得打包为可供实机使用的版本，也不支持运行时 v1 fallback。

## Task 1: 固定 v2 codec 与请求分类

**Files:** Create `App/worker/contracts.py`, `App/tests/test_dispatch_contracts.py`。Read `App/worker/protocol.py`, Spec §§5–6。

**Interfaces:** 产出接口清单中的 Context/Request/Outcome/Observation、parse_v2/encode_v2/classify，供 Tasks 2–8；此任务不切换现有主入口。

- [ ] 写以下 unittest，并加入 v1、重复 JSON key、bool epoch、负 epoch、空身份、额外 priority 字段、NaN、错误 role/方法的拒绝参数表。

```python
import unittest
from App.worker.contracts import parse_v2, classify

class ContractTests(unittest.TestCase):
    def test_zero_is_classified_by_allowlist(self):
        req = parse_v2('{"v":2,"id":"z","method":"action",'
                       '"params":{"role":"voltage","name":"zero"},'
                       '"context":{"session_id":"s","connection_id":"c","epoch":0}}')
        self.assertEqual(classify(req), 'zero')
        self.assertEqual(req.context.epoch, 0)
```

- [ ] 运行 `& $visaPython -B -m unittest App.tests.test_dispatch_contracts -v`，确认缺模块/未实现行为失败。
- [ ] 实现严格 codec，沿用现有重复 key、有限数、1 MB 请求和 64 字符 ID 限制。epoch 必须 `type(value) is int` 且非负；最大为 JS 安全整数 `9007199254740991`，溢出封锁并要求关闭，不回绕。
- [ ] 用后端精确白名单分类；PM400 任意 command 和客户端自报 priority 都不能获取安全资格。结果 `False` 是合法成功 result，不能用 truthiness 生成 ok。

```python
SAFE_ACTIONS = {('voltage', 'zero'): 'zero',
                ('gain', 'disable_current'): 'current_off',
                ('gain', 'disable_tec'): 'tec_off'}
# action 分支先校验 role/name，再查 SAFE_ACTIONS；其余允许动作均为 normal。
# disconnect/shutdown/resume 用 method 分类，未知 method 必须拒绝。
```

- [ ] 重跑目标测试和 `App.tests.test_worker_foundation`，前者全部通过，后者旧设置/枚举回归不变。增加成功/失败 Outcome JSON 往返断言和 null bootstrap 边界。
- [ ] 提交仅本任务文件：`git add -- App/worker/contracts.py App/tests/test_dispatch_contracts.py`；`git commit -m "feat: define strict console v2 contracts"`。

## Task 2: 设备级普通调度、身份与缓存

**Files:** Create `App/worker/scheduler.py`, `App/tests/test_scheduler.py`。Read `controller.py` 当前 `_lock/_status`。

**Interfaces:** 消费 Task 1；产出接口清单中的 Scheduler 公共方法。初始每 role 为 DISCONNECTED、connection_id=null、epoch=0。connect 接收后生成新 connection_id；该连接尝试的所有后续结果绑定此 ID，即使失败仍保留至释放确认。

- [ ] 写 event-controlled 测试：先 submit 并完成模拟 connect，取 `scheduler.context(role)` 构造正常请求；阻塞 OSA execute，同时确认 Voltage 普通 execute 及缓存 status 可返回。finally 中先 set 所有放行 Event，再显式 shutdown/join，防止失败测试遗留线程。

```python
import unittest
from threading import Event
from App.worker.contracts import Context, Request, Observation
from App.worker.scheduler import Scheduler

class DispatchTests(unittest.TestCase):
    def test_other_role_runs_before_osa_finishes(self):
        entered, release = Event(), Event()
        def execute(req):
            if req.method == 'action' and req.params.get('role') == 'osa':
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('test barrier not released')
            return {'connected': req.method != 'disconnect', 'result': None}
        scheduler = Scheduler(execute, lambda role, ctx: Observation({}),
                              session_id='s1')
        def send(ident, method, role, **params):
            return scheduler.submit(Request(ident, method, {'role': role, **params},
                                            scheduler.context(role)))
        try:
            send('co', 'connect', 'osa').result(2)
            send('cv', 'connect', 'voltage').result(2)
            active = send('a', 'action', 'osa', name='acquire')
            self.assertTrue(entered.wait(2))
            other = send('b', 'action', 'voltage', name='set_channel',
                         channel=1, voltage=0.1)
            self.assertEqual(other.result(2).phase, 'completed')
            self.assertFalse(active.done())
            self.assertEqual(scheduler.status()['session_id'], 's1')
        finally:
            release.set()
            scheduler.submit(Request('end', 'shutdown', {}, Context('s1', None, 0))).result(3)
            self.assertTrue(scheduler.join(3))
```

- [ ] 运行 `& $visaPython -B -m unittest App.tests.test_scheduler -v`，观察跨 role、队列上限和旧 connection 测试失败。
- [ ] 在单个 Condition 下维护 role 状态/队列和缓存修订；锁外执行 callback。每 role 固定普通线程、观察线程；全局设置/显式枚举使用一个有界管理执行槽，不能在 ingress 做文件/枚举 I/O。ping/status 不依赖管理槽，shutdown 不排在 inventory 后。
- [ ] 实现一个 active + 一个 pending，额外正常提交立刻返回 `rejected_before_call`；所有 context 在接收和 callback 前复查。保留 session/connection/epoch/revision 及活动请求 ID，发布结果深拷贝。

```python
# 检查必须在调度 Condition 内；invoke 必须在它之外。
if req.context != current_context:
    outcome = Outcome('superseded_before_call', current_context,
                      error={'type': 'StaleContext', 'message': 'context changed'})
# 只有通过检查的请求才记作 active；之后即使尚未进入驱动也计入关断屏障。
```

- [ ] 观察槽最多一个合并意图；普通动作与观察在单次读取边界交接，Gain wait_stable 是唯一允许的长普通并行观察。`status()` 不调用 execute/observe，也不读取驱动 state 属性。基础 shutdown 先封锁接收并使 pending 作废，已放行的纯 callback 退出后停观察/普通线程；join 不偷偷发布新的关闭。
- [ ] 补测试：同资源重连旧 ID 拒绝、旧 revision 不发布、调用参数外部变更不影响已接收请求、缓存拷贝不能被调用者修改、无连接启动不执行观察。重跑 Tasks 1–2。
- [ ] 提交：`git add -- App/worker/scheduler.py App/tests/test_scheduler.py`；`git commit -m "feat: add bounded per-role dispatch and cached status"`。

## Task 3: 迁移 controller 所有权和公共 API 适配

**Files:** Modify `App/worker/controller.py`, `App/tests/test_worker_controller.py`, `App/tests/test_worker_pm400.py`；extend `App/tests/test_scheduler.py`。

**Interfaces:** 消费 Scheduler；`_execute(Request) -> dict` 和 `_observe(role, Context) -> Observation` 是 Controller 的私有 callback。现有 `_action` 的公共驱动映射、PM400 registry 不改变；同步 handle 通过当前 context 构造 Request 并等待 submit 的同一个结果。创建/释放与基础 close 在本任务可测试；抢占式关闭保证由 Task 4 增加。

- [ ] 在 `test_worker_controller.py` 加入并发连接测试：同一 canonical VISA 或 COM4/`\\.\COM4` 别名只允许一个 factory 构造；不同资源可同时进入 connect。使用已有 FakeVisa/FakeVoltage 并注入 `port_enumerator=lambda: ()`。
- [ ] 运行 `& $visaPython -B -m unittest App.tests.test_worker_controller App.tests.test_scheduler -v`；确认当前全局锁下跨设备断言失败，保存 RED 输出。
- [ ] 将 `_devices/_resources` 保护缩为短时注册表锁。reserve → 构造并登记对象 → 锁外 connect → 按上下文发布；串口角色和 Fiber 的元数据保留检查在同一注册事务中提交，不能先检查后无锁登记。

```python
# 生命周期顺序，不把 connect 包在注册表锁内。
with self._lock:
    self._reserve(role, resource, context)
device = self._construct(role, resource)
with self._lock:
    self._retain(role, device, context)
if role != 'fiber':
    device.connect()
# 发布 READY 前再次检查：若 stop 已接收，只转到同一清理责任。
```

这三个私有方法的签名为 `_reserve(role, resource, context) -> None`、`_construct(role, resource) -> object`、`_retain(role, device, context) -> None`。Fiber classmethod factory 返回即已连接，不再二次 connect。factory 抛错而未返回对象时，不能伪造可重试设备；保留失败保留项并区分“无返回对象/释放未知”。
- [ ] `_action` 拆开参数验证、公共调用、编码、后续观察发布。保留原有“调用开始后异常即未知”“调用完成但编码/读回失败”的区别；不要在控制台锁内访问 PM400 capability 属性或 Fiber status。
- [ ] `_observe` 在观察执行槽读取公共状态，返回 Observation；OSA/Voltage/PM400/Fiber 保留当前字段语义，Gain 在 Task 5 接管。把旧全局 `_status()` 改为 scheduler 缓存汇总；每次调用后的缓存发布与 connection/epoch 校验绑定。普通请求的必要 post-readback 通过同一观察槽完成；失败返回 completed_readback_failed，不令 status 查询执行它。
- [ ] 保留并重跑已有不释放/状态矛盾/释放证据读取失败测试，以及 PM400 全目录测试；补 connect 尚未完成时 stop 不发布可操作 READY 的 event 测试。Task 4 尚未实现前只测试停止锁存入口，不宣称完整关闭已通过。
- [ ] 提交：`git add -- App/worker/controller.py App/tests/test_worker_controller.py App/tests/test_worker_pm400.py App/tests/test_scheduler.py`；`git commit -m "refactor: isolate controller ownership from device calls"`。

## Task 4: 安全意图、最终关断屏障与逐角色清理

**Files:** Create `App/worker/safety.py`, `App/tests/test_safety_dispatch.py`；Modify `scheduler.py`, `controller.py`。

**Interfaces:** `SafetyCoordinator` 由每 role Scheduler 持有。`request(intent: str, request_id: str) -> Future[Outcome]`、`normal_finished() -> None`、`snapshot() -> dict`。协调器接收注入的 `invoke(intent: str) -> dict`、`normal_active() -> bool` 和唤醒回调；只调度公共 API，不持有/暴露驱动对象。状态为 REQUESTED、INITIAL_RUNNING、WAITING_OLD、FINAL_RUNNING、STOP_HELD、CLOSING、RETAINED、RELEASED。

- [ ] 写关键屏障测试：自制 fake 记录 calls；普通动作在入口屏障停住；zero 第一次先执行；断言安全 Future 未完成；放行普通动作写 nonzero；断言最终 calls 的最后一项是 zero，之后才完成安全 Future。

```python
import unittest
from threading import Event
from App.worker.contracts import Request
from App.worker.controller import ConsoleController
from App.worker.simulation import SimVoltage

class FinalZeroTests(unittest.TestCase):
    def test_final_zero_follows_older_entered_write(self):
        entered, release, first_zero = Event(), Event(), Event()
        calls = []
        class PausedVoltage(SimVoltage):
            def set_channel(self, channel, voltage):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('test release missing')
                calls.append('nonzero')
                super().set_channel(channel, voltage)
            def zero(self, emergency=False):
                calls.append('zero')
                super().zero(emergency=emergency)
                first_zero.set()
        controller = ConsoleController(simulate=True, factories={'voltage': PausedVoltage})
        def action(ident, name, **params):
            return controller.submit(Request(ident, 'action',
                {'role': 'voltage', 'name': name, **params}, controller.context('voltage')))
        try:
            controller.handle('connect', {'role': 'voltage', 'resource': 'SIM-VOLTAGE',
                                          'acknowledge_lifecycle': True})
            calls.clear()
            first_zero.clear()
            old = action('old', 'set_channel', channel=1, voltage=0.1)
            self.assertTrue(entered.wait(2))
            stop = action('stop', 'zero')
            self.assertTrue(first_zero.wait(2))
            self.assertFalse(stop.done())
            release.set()
            old.result(2)
            self.assertEqual(stop.result(2).phase, 'completed')
            self.assertEqual(calls, ['zero', 'nonzero', 'zero'])
        finally:
            release.set()
            controller.close()
```

- [ ] 运行 `& $visaPython -B -m unittest App.tests.test_safety_dispatch -v`，确认旧实现无法满足屏障及排队作废。
- [ ] 接收新的安全请求即推进 epoch、封锁普通入口并把旧 pending 完成成 superseded；立即调用白名单，绝不先等 active 退出。活跃线程数量固定，不为重复点击增线程。相同意图的新 ID 立刻返回 rejected_before_call，error 的 type=AlreadyRunning、attempt_id 指向现有责任；不另加 waiter、重复推进 epoch 或伪称动作完成。前端继续查看原 attempt。
- [ ] 用明确强度表升级安全责任。初始调用正在做 I/O 时不能启动第二个同 role 安全 I/O；等待 OLD 阶段只保留状态、不占执行线程，升级 close 能立即被调度。

```python
INTENT_ORDER = {'gain': ('current_off', 'tec_off', 'disconnect'),
                'voltage': ('zero', 'disconnect')}
# newer 更强时更新 effective_intent；所有旧请求结果注明这个最终意图。
# 若 normal_active() 为真：第一次安全调用返回后等通知，再运行最终安全调用。
# 若升级为 disconnect：利用公共 close 的 terminal 状态，不假装为取消稳定等待。
```

- [ ] 每 role 建立并保留一个 `InstrumentSession(**{role: device})` 清理责任；全局 shutdown 同时提交各 role 清理，不按五角色串行等待。Gain 保留 current-off → TEC-off → close；Voltage zero → close；PM400 等活动操作后 close；Fiber setup.close，禁止零/回滚。
- [ ] 保存每次清理尝试冻结结果和独立 attempt_id。只有 release 证据确认且该 role 所有活动普通/观察/安全调用退出才释放保留。EOF 且清理未完保留进程和诊断，不能承诺失去的 stdin 仍可重试。
- [ ] 实现 `resume`：精确 context、confirm=true、已最终完成、观察确认健康、无旧任务才解除 STOP_HELD；只改调度状态，零驱动写入。失败/未知不允许恢复，状态读取成功也不自动恢复。
- [ ] 补矩阵并重跑：Gain False 成功、current-off 升 TEC-off 升 close、稳定等待不被冒称取消、关断失败再成功保留两份报告、blocked PM400 不拖延其它关断、Fiber 没有 zero 调用、connect 与 shutdown 竞争、重复 close 只一次活动调用。所有 fake barrier 在 finally 放行。
- [ ] 提交本任务五个文件：`git add -- App/worker/safety.py App/worker/scheduler.py App/worker/controller.py App/tests/test_safety_dispatch.py App/tests/test_scheduler.py`；`git commit -m "feat: prioritize safe-off with terminal outcome barriers"`。

## Task 5: Gain 字段证据与真实语义的模拟器

**Files:** Create `App/worker/observations.py`, `App/tests/test_observations.py`；Modify `controller.py`, `scheduler.py`, `simulation.py`, `test_worker_controller.py`。

**Interfaces:** EvidenceStore（接口清单）；字段固定为 temperature_c、target_c、tec_enabled、current_ma、current_enabled。record 只接受当前 connection 的非过期观察修订；invalidate 保留历史 value/原时间但 quality=unknown；fail 保留历史并 quality=error；snapshot 用当前单调时间生成 age，不更新采样时间。

- [ ] 写独立假时钟测试，先给五字段赋值，推进六秒后只刷新温度，确保另四字段不是 fresh。保存原值供显示，但不得把它们转换成 Off。

```python
now = [10.0]
store = EvidenceStore('c1', clock=lambda: now[0])
store.record('tec_enabled', True, started_at=10.0, revision=1)
now[0] = 16.0
store.record('temperature_c', 24.0, started_at=16.0, revision=2)
snapshot = store.snapshot()
assert snapshot['tec_enabled']['quality'] == 'stale'
assert snapshot['temperature_c']['quality'] == 'fresh'
```

- [ ] 运行 `& $visaPython -B -m unittest App.tests.test_observations -v` 观察失败。
- [ ] 实现数据记录以及按请求开始时间计算 age；无成功观察为 unknown，严格 `age > 5.0` 才 stale。非有限值/未来开始时间/错误修订不得刷新证据；原始时间不跨 IPC 暴露为可比较客户端时间。
- [ ] controller 用五个公共 fresh reader 逐字段读取，不用 read_status 的公共 timestamp。连接后、普通动作后、2.5 s 定时合并刷新；字段间检查 stop/普通任务，不能用五字段大锁。失败字段只标自身 error；发现联锁故障同时使两个开关失效，记录公共异常后停止本轮观察。

```python
READERS = {'temperature_c': 'read_temperature', 'target_c': 'read_target',
           'tec_enabled': 'read_tec_enabled', 'current_ma': 'read_current',
           'current_enabled': 'read_current_enabled'}
AFFECTED = {'set_temperature': ('target_c',), 'set_current': ('current_ma',),
            'enable_tec': ('tec_enabled',),
            'disable_tec': ('tec_enabled', 'current_enabled'),
            'enable_current': ('current_enabled', 'current_ma'),
            'disable_current': ('current_enabled',)}
# 在驱动调用开始前 invalidate(AFFECTED[name], 'command_started')。
# 任何 fault/不确定失效都不可恢复旧 fresh；需新的独立读回。
```

- [ ] 正常观察与写入按单字段边界交接；安全动作推进 epoch 后正在返回的旧观察不发布。STOP_HELD 最终安全完成后恢复观察，但不解除写封锁。Gain wait_stable 可同时观察，仍由驱动 request lock 管串口。
- [ ] 修改 SimGain：read_status 不更新 timestamp；补五个 reader；disable_tec 先 disable_current；enable_current 模拟开启后设定改变但适配器一律通过 read_current 取值；保持五秒稳定要求。用假时钟断言两次 cached read 的 received_at 相等。
- [ ] 补测试：写后 old On 失效、enable 后不沿用 pre-enable 电流、False 为确认关闭、读开关导致 safety trip 不能发布成功读回、部分刷新失败、连接初始缓存年龄未知、观察晚于新写入返回、温度线程仍不自动开电流。
- [ ] 重跑 `App.tests.test_observations App.tests.test_worker_controller App.tests.test_safety_dispatch`；提交：`git add -- App/worker/observations.py App/worker/controller.py App/worker/scheduler.py App/worker/simulation.py App/tests/test_observations.py App/tests/test_worker_controller.py`；`git commit -m "fix: track Gain readback evidence per field"`。

## Task 6: Rust 并发 broker、有界优先发送和生命周期

**Files:** Create `App/src-tauri/src/reply_broker.rs`, `request_writer.rs`；Modify `worker.rs`, `main.rs`, `App/tests/native_worker_fixture.py`。v2 握手的生产入口切换留 Task 7。

**Interfaces:** broker 独立于 Child，`ReplyBroker::new()`、`register(id: &str, role: Option<&str>) -> Result<Receiver<Value>, String>`、`deliver(frame: Value) -> Result<(), String>`、`mark_timeout(id: &str)`、`transport_failed(message: &str)`。writer 为 `enqueue(frame: Value, class: DispatchClass) -> Result<(), String>`；DispatchClass 在 request_writer.rs 定义为 Normal、Query、Safety(String)、Shutdown，由 Rust 校验白名单生成。WorkerState 最终持有 `Mutex<Option<Arc<Worker>>>`；Child、writer 和 broker 各自独立短锁。

本任务的新责任类型名固定为 `WorkerRuntime`：`spawn(python: &Path, root: &Path, mode: &str) -> Result<Arc<Self>, String>`、`exchange(&self, request: &Value, timeout: Duration) -> Result<Value, String>`、`shutdown(&self) -> Result<Value, String>`。Task 7 用 `type Worker = WorkerRuntime` 统一迁移并删除旧 Worker 实现；不是同时运行两个 Python 进程。

- [ ] 在 broker 模块单测中注册 a/b 后逆序 deliver；使用下面完整测试形态，另加重复 ID、合法迟到回复、256 项终态淘汰、超时记录仍占槽。

```rust
#[test]
fn reverse_replies_reach_their_own_waiters() {
    let broker = ReplyBroker::new();
    let a = broker.register("a", Some("osa")).unwrap();
    let b = broker.register("b", Some("voltage")).unwrap();
    for id in ["b", "a"] {
        broker.deliver(serde_json::json!({"v":2,"id":id,"ok":true,
            "phase":"completed","context":null,"result":{}})).unwrap();
    }
    assert_eq!(a.try_recv().unwrap()["id"], "a");
    assert_eq!(b.try_recv().unwrap()["id"], "b");
}
```

- [ ] 从 `App/src-tauri` 运行 `cargo test --offline --locked reply_broker`（工具链初始化见 Task 10）；观察新增行为失败。
- [ ] 实现一条 stdout reader 路由，不再在 exchange 中吞掉其它 ID。无匹配且不在终态环中的回复、非法 envelope、版本错误和 EOF 标传输未知；不能把错误当作另一个请求成功。mark_timeout 只标原 role 受限，后续 status 成功不解除。
- [ ] writer 单线程拥有 stdin，使用 Mutex + Condvar + 有界队列；普通类总共 32 槽，其中 1 槽保留给合并的 status/ping Query，最多 31 个其它普通请求，确保队列满仍可检查状态。另有每 role 安全槽、全局 shutdown 槽；Python admission 使用相同限额。同意图重复返回“已有 attempt”并引用其 ID，不另建无界 waiter；更强意图替换尚未发送槽，若已发送则用保留控制责任送达升级，不等待旧请求终态。将旧请求与升级结果的实际 effect 保持关联。
- [ ] Writer 单测用内存 Write fake 记录完整行，测试普通队列满仍可放安全意图；阻塞 Write fake 时，只暴露“未确认送达”，不能跳过单写入器直接写 stdin。所有者锁不得跨 write/recv_timeout/try_wait 循环。
- [ ] 拆宿主 shutdown 协调：唯一 attempt ID；活进程失败清理允许显式新尝试，已释放只等待进程退出；Drop 不开启第二次并发 close，不调用 Child.kill。保留 confirmed abnormal exit + original report 的两个证据。
- [ ] 扩展标准库 fixture，精确 `--simulate`，以现有测试专用 `configure` 方法配置延迟/逆序/释放屏障；此方法只存在 fixture、不加入产品 allowlist。Task 6 的新 broker 测试可给 fixture 显式测试参数 `--protocol 2`，旧测试仍用其既有 v1；此双版本能力只在测试 peer，Task 7 后删除 v1 fixture 分支。用真实管道测试 stop 不等 OSA 模拟回复。
- [ ] 新宿主运行责任以 `WorkerRuntime` 在 worker.rs 内独立测试；Task 7 再让 WorkerState 持有它并删除旧 exchange。重跑旧 worker/startup 测试不改变其版本期望；另用新 Runtime 测试证明 status 成功不解除未知。Task 7 才迁移旧 `successful_status_restores_request_flow_after_timeout` 为“不自动恢复”，保留失败关闭、cached cleanup 后延迟退出覆盖。不要让此任务出现新旧协议混用导致的整套回归失败。
- [ ] 提交：`git add -- App/src-tauri/src/reply_broker.rs App/src-tauri/src/request_writer.rs App/src-tauri/src/worker.rs App/src-tauri/src/main.rs App/tests/native_worker_fixture.py`；`git commit -m "feat: add correlated bounded worker transport"`。

## Task 7: 原子切换 v2 ingress、宿主、客户端与测试夹具

**Files:** Modify `App/worker/main.py`, `protocol.py`, `App/src-tauri/src/worker.rs`, `startup_handshake.rs`, `App/web/api.js`, `App/web/main.js`；Modify `test_worker_foundation.py`, `test_worker_process.py`, `host.test.mjs`, `native_close_fixture.py`, `test_native_close_fixture.py`, `preview_server.py`, `preview-bridge.js`, `test_preview_server.py`, `test_package_layout.py`；迁移其它测试中的 wire/handshake v1 字面量，保留 settings v1。

**Interfaces:** `protocol.py` 唯一导出严格 v2 parser/encoder；main.run 把 controller.submit Future 的终态交给一个有界回复写入器。`createClient(invoke)` 保留现有方法，增加 `request(method, params={}, context=null)`，返回 `{requestId, phase, context, result}`；错误对象保留 phase/context/requestId 而非只有字符串。同步迁移 main.js 请求包装读取 `.result`、握手缓存 role contexts、发送时传入 context；Task 8 再改变 busy/快照门控，不能把整个调用端迁移拖到 Task 8 才使 Task 7 测试通过。

- [ ] 新增进程测试：连续写 acquire、zero、status 三行，不等第一行回复；按 ID 收集，确认 zero/status 在 OSA 屏障释放前返回。测试 peer 控制屏障，产品 worker 不加调试命令。
- [ ] 运行 `App.tests.test_worker_process` 和 `node --test App/tests/host.test.mjs`，记录当前串行/版本不匹配失败。
- [ ] main 输入循环只 parse/submit/注册结果，不调用 Future.result。回复 writer 使用独立固定线程；Future callback 只 put_nowait 到有界队列。按全局 admission 上限预留每个接受请求一个终态槽；输出不通时停止普通 admission并请求清理，但 ingress 不能阻塞在 reply put。

```python
# ingress 的核心边界；done callback 不进行设备 I/O。
future = controller.submit(request)
future.add_done_callback(
    lambda completed, request_id=request.id:
        reply_queue.put_nowait((request_id, completed.result()))
)
# writer 独占 output_stream.write/flush，所有日志仍重定向 stderr。
```

对 parse 失败的重复恶意输入，不无限积累 error frame：使用固定一个协议错误回复槽；耗尽则标协议故障并发布清理，保留已接受操作的终态证据。全局 pending/终态跟踪上限与 Task 6 一致，不用无界 Queue。

submit 返回的 Future 以 Outcome 表示设备失败，不把设备异常直接留在 Future 中；意外 callback 异常记录协议故障并触发同一清理协调器，不能静默丢失已接受请求。总 admission 容量计入尚未发送的回复，不在 Future 一完成就释放其容量，避免输出阻塞时回复队列被后续请求淹没。
- [ ] shutdown 失败且未释放时继续服务 status/显式重试，不能 break 后无条件 finally 再清理同一实例。成功关闭等待有关线程结束、flush 报告后退出；EOF 失败保留诊断并进入 retained wait，不自旋、不声称可重试。
- [ ] 一次切换 Rust handshake/request 默认版本、Python protocol、UI client。ping 返回 session_id 与 role contexts；启动/重载读身份后才允许有上下文的动作。host 生成不重复的 ping/shutdown ID，不能继续用固定 host-shutdown。
- [ ] 预览桥仅持有短时 owner/settings 锁，锁外等待 submit；删除整个 invoke 外层串行锁。保持 simulation-only，不增加外部监听权限。包布局测试改为先握手再生成带 session 的 shutdown，不依赖回复顺序等于请求顺序。
- [ ] 迁移 `native_close_fixture.py` 的 v2 身份并保留标准库-only、拒绝真实模式、返回故障后 exit19。扫描 `rg -n 'protocol_version|"v": ?1|v: 1|Some\(1\)' App` 逐项区分协议与设置/数据值，不做全仓盲替换。
- [ ] 运行全部 App Python、Node、Rust 测试；检查 v1-only worker 明确拒绝而不是降级，新 startup 仍零设备、零工厂调用。提交以上实际修改文件，消息 `feat: switch console stack to nonblocking protocol v2`。

## Task 8: 按设备交互、状态防倒退与 Gain 显式开关

**Files:** Create `App/web/control-state.js`, `App/tests/control-state.test.mjs`；Modify `main.js`, `panels.js`, `view-model.js`, `operations.js`, `lifecycle.js`, `main.test.mjs`, `host.test.mjs`, `ui.test.mjs`；必要时 `style.css`。

**Interfaces:** 纯函数 `canApplySnapshot(current, incoming) -> boolean`、`fieldAge(field, roundTripMs, receivedAtMs, nowMs) -> number|null`、`canSendNormal(roleState) -> boolean`、`canSendSafety(roleState) -> boolean`；main 持有每 role 的 `{normalPending,safetyPending,context,revision,mode}`，mode 包含 STOP_HELD/UNKNOWN/CLOSING。不把局部 UI busy 当硬件权限。

- [ ] 写纯函数回归与生产 main fake DOM 回归：未返回的 OSA action 不阻止 zero/current-off；同 role 普通仍禁止；同时调用 intercepted poll，必须发出 status。沿用现有动态 import 和 finally 恢复 globals 的 harness，不建立第二套假 UI。

```javascript
import assert from 'node:assert/strict';
import test from 'node:test';
import { fieldAge } from '../web/control-state.js';
test('transit time is included in conservative field age', () => {
  const age = fieldAge({observed_age_s: 1}, 4000, 10000, 11000);
  assert.equal(age, 6);
});
```

- [ ] 运行 `& $nodeExe --test App/tests/control-state.test.mjs App/tests/main.test.mjs App/tests/ui.test.mjs`，记录全局 busy/错误年龄/unknown-as-off 的失败。
- [ ] 实现快照判断：session 不同只允许显式握手采纳；connection 不同按当前生命周期采纳新实例，旧实例永不覆盖；相同身份要求 epoch 不倒退、revision 严格递增。OSA trace、遥测 trend、draft 与按钮权限都经过相同门控，不只拦截总览。

```javascript
export function fieldAge(field, roundTripMs, receivedAtMs, nowMs) {
  if (!Number.isFinite(field?.observed_age_s) ||
      ![roundTripMs, receivedAtMs, nowMs].every(Number.isFinite)) return null;
  return Math.max(0, field.observed_age_s) + Math.max(0, roundTripMs) / 1000 +
    Math.max(0, nowMs - receivedAtMs) / 1000;
}
```

- [ ] 将全局 busy 拆为 worker 生命周期与 role normal/safety 状态；断开/全局关闭仍可触达，不在 UI 因驱动 busy 提前拒绝。安全重复点击附着原 attempt；更强意图仍发送。显示“请求中/已接收/等待旧调用/最终完成/未知”，不使用“硬件已急停”文案。
- [ ] Gain 按字段显示 value、age、quality；undefined/null 显示 Unknown。渲染固定 Enable/Disable 按钮并 dispatch 固定 name，不用旧布尔值反转；开启电流需五字段 fresh、TEC On、当前身份和权限有效，驱动仍作最终稳定判断。
- [ ] 添加 STOP_HELD 的显式恢复普通控制按钮，带 confirm=true/context；不能自动重放输入。页面重载只读，先重新确认控制上下文；Fiber 旧页面上下文/旧基线不能凭相同 serial 或资源名恢复。正常重载不向 setup 写新基线。
- [ ] 关闭 progress 展示逐 role attempt；失败窗口保留原始报告。增加 UI 测试：reversed responses、同资源 reconnect 清草稿/trace、Gain wait 完成不自动 enable、False 正确 Off、旧 status 不解锁、失败 close 后二次明确操作。
- [ ] 运行全部 Node 与 App Python；提交上述实际改动文件，消息 `feat: expose per-device safety state and truthful Gain status`。

## Task 9: 跨层离线验收与故障回归

**Files:** Create `App/tests/test_dispatch_e2e.py`；extend `test_worker_process.py`, `test_package_layout.py`, `test_preview_server.py`, Rust `worker.rs` tests；update `docs/acceptance.md`。

**Interfaces:** 不新增产品方法。以现有 production Controller + 模拟/注入 fake、真实 Rust 管道 broker、生产 frontend 模块组合验证；专用 fixture 永远不导入真实设备。

- [ ] 写端到端场景测试：held OSA + Voltage ramp/zero + Gain wait/current-off + status；在放行 OSA 前记录安全 callback 已进入。零动作 final Future 仅在旧调用退出后完成。
- [ ] 写关闭重试测试，保存报告 deep copy，对第二次尝试完成后做严格相等比较：

```python
from copy import deepcopy
first_report = deepcopy(first_outcome.result)
# 显式提交第二个 shutdown Request，使用新的 request ID。
assert second_outcome.result['attempt_id'] != first_report['attempt_id']
assert first_outcome.result == first_report
# 不是用第二次结果覆盖 first_outcome。
```

- [ ] 首先运行 `& $visaPython -B -m unittest App.tests.test_dispatch_e2e -v`。若现有实现即通过，应临时在测试 fake 中取消屏障/提供旧 context 的负对照，证明断言会拒绝错误顺序；不得为制造 RED 修改真实驱动。
- [ ] 覆盖 EOF、stdout 阻塞、worker异常退出、半连接、PM400持锁、重复安全点击洪泛、队列满、断线重连旧上下文；固定限额保持，清理 attempt 不与后台观察竞争发布假 released。
- [ ] 跑完整回归：

```powershell
& $visaPython -B -m unittest discover -s App/tests -p 'test_*.py' -q
& $visaPython -B -m unittest discover -s Code/Debugs -p 'test_*.py' -q
& $nodeExe --test App/tests/*.test.mjs
git diff --check
```

另在初始化后的 `App/src-tauri` 运行 `cargo test --offline --locked`、`cargo fmt --check`。逐条记录退出码、实际计数和耗时；失败时不得继续打包称完成。
- [ ] 按 Spec §10 的每行验收矩阵列出实际测试名；仅在 `docs/acceptance.md` 写本次实测事实，不覆盖旧历史结果，不把软件 fake 当实机。检查 `git diff --name-only 0c9e804 -- Code Config reference_code` 应无产品修改；若出现需解释/撤回自己越界修改而不动用户改动。
- [ ] 提交新回归和验收记录，消息 `test: verify end-to-end console safety dispatch`。

## Task 10: 原生模拟验收、打包与交付

**Files:** Modify `App/README.md`, `docs/acceptance.md`；生成输出只放 `D:/Qian/Codex_Project/SIL_Experiments/Result/console`，临时构建沿用 D 盘 `tmp/tauri-build`。不修改驱动，不覆盖用户正在运行的安装目录。

**Interfaces:** 交付包含同一提交的 EXE、完整 App/Code/Config 资源和验收报告；无半套 v1/v2 资源混用。

- [ ] 初始化已有本机 MSVC/Rust；不重装、不 cargo install、不升级 lockfile：

```powershell
& 'D:/SoftwareInstaller/MicrosoftBuildTools2022/Common7/Tools/Launch-VsDevShell.ps1' -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
$buildRoot = 'D:/Qian/Codex_Project/SIL_Experiments/tmp/tauri-build'
$env:RUSTUP_HOME = "$buildRoot/rustup"
$env:CARGO_HOME = "$buildRoot/cargo"
$env:CARGO_TARGET_DIR = "$buildRoot/target"
$env:TEMP = "$buildRoot/temp"
$env:TMP = $env:TEMP
$env:PATH = "$buildRoot/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin;$env:CARGO_HOME/bin;$env:PATH"
```

- [ ] 在 `App/src-tauri` 先执行 `cargo test --offline --locked`、`cargo fmt --check`，再 `cargo tauri build --ci -- --locked`。缺本地依赖则记录阻碍，不擅自安装或切换解释器。
- [ ] 核对 bundled resources；运行 v2 package-layout/缺资源负对照。将新产物放独立、未使用的模拟验收目录；记录完整路径、提交、SHA256。只启动自己这次创建的 simulation 实例，禁止连真实设备或争用用户正在浏览的窗口。
- [ ] 桌面输入可访问后执行原生模拟清单：OSA采集期间操作Voltage归零/Gain关电流、Gain显式开关/年龄、PM400功能门控、单/双Fiber连接与权限、STOP_HELD恢复、迟到结果、窗口关闭取消/成功/失败。阻塞场景用标准库测试peer，不向产品加入调试后门。
- [ ] 核对“安全请求已发送但旧调用未退出”不会显示最终成功；失败清理 + 活进程重试，以及故障报告 + exit19 都保留原报告。不要求在未获授权时验证实际响应时间或物理零。
- [ ] 若 GetCursorPos/桌面访问失败，停止 UI 操作、保留未验收项并询问桌面可用性；不重复安装/强杀/接管其它窗口作为替代。软件测试已通过也不能勾选此项。
- [ ] 更新 README 的单工作进程、v2、按 role 调度、关断最终屏障、Gain读取联锁副作用和未知状态语义。更新 acceptance 的各层结果；复制已验证构建到 Result/console，并运行以下产物哈希核查；目录中多个安装包时先人工核对，不按“最新文件”猜测。

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath "$buildRoot/target/release/sil-instrument-console.exe"
$setupCandidates = @(Get-ChildItem -LiteralPath "$buildRoot/target/release/bundle/nsis" -File -Filter '*-setup.exe')
if ($setupCandidates.Count -ne 1) { throw 'Select and verify the exact installer from the build output first' }
Get-FileHash -Algorithm SHA256 -LiteralPath $setupCandidates[0].FullName
```
- [ ] 请求一次完整变更的独立审查，解决重要问题并重跑相关测试；最后 `git diff --check`、`git status --short`。提交文档及必要测试修正，消息 `docs: record safety-dispatch native acceptance`；不合入 main 或推送。

## 依赖、审查与执行选择

Task 1 → Task 2 → Task 3 → Task 4；Task 5 使用 Tasks 2–4。Task 6 可以与 Task 5 独立准备，但 Task 7 必须等 Tasks 1–6 通过，之后 Task 8 → Task 9 → Task 10。共享 scheduler/controller/worker 文件的实现不可同时让两个 agent 修改。

这是一条相互依赖的安全调度迁移，不拆成可独立发布的“只改 UI”或“只改锁”版本。每个任务可单独测试和提交，但完整原生验收之前不能宣称已可用于实验。

用户须先审阅本计划，再确认执行方式。推荐逐任务实现并独立审查，因为错配回复、错误关断终态或过期状态放行都可能影响真实输出；也可沿用本会话由主 agent 逐项实施、最后进行一次独立总审查，减少交接开销。

## 计划自审覆盖记录

- Spec §§1–4：范围/驱动边界由 Global Constraints、Tasks 3/10 和最终 diff 检查覆盖。
- Spec §5：身份/协议/限额/结果由 Tasks 1/2/6/7 覆盖。
- Spec §§6–7：队列作废、最后关断、升级、资源保留和重试由 Tasks 2/3/4/6/9 覆盖。
- Spec §8：逐字段来源、年龄、失效、读取副作用与模拟语义由 Task 5/8 覆盖。
- Spec §9：按角色交互、迟到结果、重载和关闭 UI 由 Task 8/10 覆盖。
- Spec §§10–11：分层验收、原生访问门槛、实机另授权、不得自动合入由 Tasks 9/10 覆盖。
- Review Focus 五项已分别在对应任务列为回归，不将它们仅留给人工审查。

计划状态：待用户审阅/选择执行方式。以上复选框均为未来工作，本轮没有执行产品测试、修改程序或操作硬件。
