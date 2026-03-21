# 计算机视觉

Canal Engine 包含计算机视觉引擎（`canal-cv`），用于基于屏幕的自动化。它可以检测 UI 元素、按坐标执行操作、录制和回放工作流。

## 架构

```
屏幕捕获 → 视觉检测 → 操作规划 → 操作执行 → 验证
```

### 组件

| 组件 | Trait/类型 | 描述 |
|------|-----------|------|
| 屏幕捕获 | `ScreenController` | 从桌面或浏览器捕获截图 |
| 元素检测 | `VisionDetector` | 检测 UI 元素及其边界框 |
| 操作流水线 | `ComputerUsePipeline` | 编排观察 → 检测 → 执行循环 |
| 操作执行器 | `ActionChainExecutor` | 执行点击、输入、滚动、拖拽序列 |
| 屏幕监控 | `ScreenChangeDetector` | 实时屏幕变化检测 |
| 工作流录制 | `WorkflowRecorder` | 录制用户操作以供回放 |

## 视觉检测器

### OmniParser (ONNX)

本地 ONNX 元素检测，无需 API 调用。

设置：

```bash
# 下载 ONNX 模型
mkdir -p models/omniparser
# 将模型文件放入目录

# 设置环境变量
OMNIPARSER_MODEL_DIR=models/omniparser
```

启用 OmniParser 构建：

```bash
cargo build -p gateway-api --features omniparser
```

### UI-TARS

基于云的视觉模型，用于 GUI 自动化。通过 OpenRouter 使用。

```bash
OPENROUTER_API_KEY=sk-or-v1-your-key
```

在 `config/uitars.yaml` 中配置。

### Molmo

替代视觉模型客户端。

## 操作类型

CV 引擎支持以下操作：

| 操作 | 描述 |
|------|------|
| `click` | 在坐标 (x, y) 处点击 |
| `double_click` | 双击 |
| `right_click` | 右击 |
| `type` | 在当前光标位置输入文本 |
| `scroll` | 上下左右滚动 |
| `drag` | 从 A 点拖拽到 B 点 |
| `key` | 按键盘按键或快捷键 |
| `wait` | 等待指定时间 |
| `screenshot` | 捕获当前屏幕状态 |

## 工作流录制

录制用户操作并回放：

```bash
# 开始录制
curl -X POST http://localhost:4000/api/workflows/record/start \
  -H "Authorization: Bearer TOKEN" \
  -d '{"name": "login-flow"}'

# 录制操作
curl -X POST http://localhost:4000/api/workflows/record/action \
  -H "Authorization: Bearer TOKEN" \
  -d '{"action": "click", "x": 100, "y": 200, "element": "登录按钮"}'

# 停止录制
curl -X POST http://localhost:4000/api/workflows/record/stop \
  -H "Authorization: Bearer TOKEN"
```

### 工作流模板

录制的工作流可以泛化为模板：

- 变量提取（URL、表单值变为参数）
- 步骤标注（人类可读描述）
- 错误恢复（每步的重试/跳过策略）

```bash
# 列出模板
curl http://localhost:4000/api/workflows/templates \
  -H "Authorization: Bearer TOKEN"

# 执行模板
curl -X POST http://localhost:4000/api/workflows/templates/login-flow/execute \
  -H "Authorization: Bearer TOKEN" \
  -d '{"url": "https://example.com", "username": "user", "password": "pass"}'
```

## 屏幕监控

实时屏幕变化检测，用于自动化验证：

```rust
let monitor = ScreenMonitor::new(config);
monitor.on_change(|change| {
    println!("屏幕变化: {:?}", change.region);
});
monitor.start().await;
```

## 与 Agent 集成

当任务需要屏幕交互时，Agent 使用 CV 工具：

1. Agent 截图
2. 视觉检测器识别 UI 元素
3. Agent 根据检测到的元素规划操作
4. 操作执行器执行操作
5. Agent 再次截图验证

Agent 可用的内置 CV 工具：

- `TakeScreenshot` — 捕获桌面/浏览器
- `FindElement` — 通过描述定位 UI 元素
- `OcrText` — 从屏幕区域提取文本
- `MouseClick` — 按坐标点击
- `KeyboardType` — 输入文本
- `Scroll` — 滚动
- `WaitForElement` — 等待元素出现
