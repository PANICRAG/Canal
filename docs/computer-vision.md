# Computer Vision

Canal Engine includes a Computer Vision engine (`canal-cv`) for screen-based automation. It can detect UI elements, execute actions by coordinates, and record/replay workflows.

## Architecture

```
Screen Capture → Vision Detection → Action Planning → Action Execution → Verification
```

### Components

| Component | Trait/Type | Description |
|-----------|-----------|-------------|
| Screen Capture | `ScreenController` | Captures screenshots from desktop or browser |
| Element Detection | `VisionDetector` | Detects UI elements and their bounding boxes |
| Action Pipeline | `ComputerUsePipeline` | Orchestrates observe → detect → act loops |
| Action Executor | `ActionChainExecutor` | Executes click, type, scroll, drag sequences |
| Screen Monitor | `ScreenChangeDetector` | Detects real-time screen changes |
| Workflow Recorder | `WorkflowRecorder` | Records user actions for replay |

## Vision Detectors

### OmniParser (ONNX)

Local ONNX-based element detection. No API calls needed.

Setup:

```bash
# Download ONNX models
mkdir -p models/omniparser
# Place model files in the directory

# Set environment variable
OMNIPARSER_MODEL_DIR=models/omniparser
```

Build with OmniParser support:

```bash
cargo build -p gateway-api --features omniparser
```

### UI-TARS

Cloud-based vision model for GUI automation. Uses OpenRouter.

```bash
OPENROUTER_API_KEY=sk-or-v1-your-key
```

Configured in `config/uitars.yaml`.

### Molmo

Alternative vision model client.

## Actions

The CV engine supports these action types:

| Action | Description |
|--------|-------------|
| `click` | Click at coordinates (x, y) |
| `double_click` | Double-click at coordinates |
| `right_click` | Right-click at coordinates |
| `type` | Type text at current cursor position |
| `scroll` | Scroll up/down/left/right |
| `drag` | Drag from point A to point B |
| `key` | Press keyboard key or shortcut |
| `wait` | Wait for specified duration |
| `screenshot` | Capture current screen state |

## Workflow Recording

Record user actions and replay them:

```bash
# Start recording
curl -X POST http://localhost:4000/api/workflows/record/start \
  -H "Authorization: Bearer TOKEN" \
  -d '{"name": "login-flow"}'

# Record actions (called by the CV agent during execution)
curl -X POST http://localhost:4000/api/workflows/record/action \
  -H "Authorization: Bearer TOKEN" \
  -d '{"action": "click", "x": 100, "y": 200, "element": "Login button"}'

# Stop recording
curl -X POST http://localhost:4000/api/workflows/record/stop \
  -H "Authorization: Bearer TOKEN"
```

### Workflow Templates

Recorded workflows can be generalized into templates:

- Variable extraction (URLs, form values become parameters)
- Step annotation (human-readable descriptions)
- Error recovery (retry/skip strategies per step)

```bash
# List templates
curl http://localhost:4000/api/workflows/templates \
  -H "Authorization: Bearer TOKEN"

# Execute a template
curl -X POST http://localhost:4000/api/workflows/templates/login-flow/execute \
  -H "Authorization: Bearer TOKEN" \
  -d '{"url": "https://example.com", "username": "user", "password": "pass"}'
```

## Screen Monitoring

Real-time screen change detection for automation verification:

```rust
let monitor = ScreenMonitor::new(config);
monitor.on_change(|change| {
    println!("Screen changed: {:?}", change.region);
});
monitor.start().await;
```

## Integration with Agent

The agent uses CV tools when the task requires screen interaction:

1. Agent takes a screenshot
2. Vision detector identifies UI elements
3. Agent plans actions based on detected elements
4. Action executor performs the actions
5. Agent takes another screenshot to verify

Built-in CV tools available to the agent:

- `TakeScreenshot` — Capture desktop/browser
- `FindElement` — Locate UI elements by description
- `OcrText` — Extract text from screen regions
- `MouseClick` — Click at coordinates
- `KeyboardType` — Type text
- `Scroll` — Scroll in a direction
- `WaitForElement` — Wait for an element to appear
