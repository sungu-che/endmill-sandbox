# 가짜 앤드밀 샌드박스 오픈소스 프로젝트

## 프로젝트 전제

오프라인에서 필요한 것은 다음과 같습니다:

- 이벤트 카메라
- 엔드밀, 절삭공구, 공작물
- 전문성

해당 항목은 없기 때문에 가상 시뮬레이션으로 하되, 가능하면 각 환경적인 부분을 조사하여
절삭공구 테스트가 가능하다면 첫번째로 휴대폰 카메라로 낮은 RPM 상황에서 스캔된 영상을
이벤트 카메라 포맷의 데이터로 전처리하여 마모 체크를 진행하거나, 근본적으로 가장 확실한
휴대폰 카메라나 이벤트 카메라로 해당 값을 얻는 게 가능한 희망적인 상황을 고려해서 진행하고자 합니다.

### Plan A — 카메라와 시뮬레이션 연동

**카메라 촬영 기준:**

- 앤드밀이 한 바퀴 돌면 파노라마 사진과 같이 3D로 접어서 마모 편차를 계산합니다.
- 피삭재의 경우 촬영하는 부분이 아니기 때문에 피지컬 예측으로 진행합니다.

### Plan B (선택) — 휴대폰 카메라와 이벤트 카메라 포맷 전처리 후 시뮬레이션 연동, 또는 가상 조건으로 진행

**가상환경:**

- 온도, 습도
- 앤드밀 형태, 앤드밀 코팅 소재
- 피삭재 형태와 피삭재 소재
- 냉각 방식 (절삭유 등)

**가상상호작용:**

- 앤드밀과 피삭재 간의 스트레스 증가
- 냉각으로 인한 앤드밀의 스트레스 감소
- 냉각으로 인한 피삭재의 스트레스 감소

**휴대폰 시나리오 가설:**

휴대폰 촬영 가능한 조건은 1000 RPM 근방에서 가능하며:

0. 최초에 앤드밀의 위치를 포커싱합니다.
1. 가공 중에 G-코드로 RPM과 이송 속도를 변경하는 형태로, 원하는 시점에 RPM과 이송 속도 값만 바꿔주면 스핀들이 즉시 속도를 줄여서 그 시점에만 촬영을 진행하여 앤드밀 상태를 체크하고 랩탑으로 전송합니다.
2. 랩탑에서는 Depth Estimation으로 깊이를 잡고, mask-generation 모델로 형태를 정확하게 선택하여 두 모델 결과 값을 기준으로 기준 값을 잡습니다.
3. 이후 촬영에서 최초 원본 기준 값에서 변화되는 특징을 잡습니다.
   - 특징 값을 쌓은 뒤 시계열 모델로 현재 시간 싱크가 맞지 않는 부분을 잡아보고 싶습니다.

> ⚠️ 해당 가설은 절대 정확하지 않으며, 절삭공구가 없어서 가설에서 끝날 가능성이 높습니다.

### 공통

- 앤드밀 브랜드는 저작권 이슈가 있어서 가져올 수 없으며, 가능하면 해당 앤드밀 형태와 앤드밀 코팅 소재를 프리셋으로 가져오는 방향으로 하고자 합니다.

---

## End Mill Sandbox — Project Premise (English)

### What This Project Assumes Offline

The following are required for real-world testing:

- Event camera
- End mills, cutting tools, workpieces
- Domain expertise

None of these are available. Therefore, this project proceeds as a **virtual simulation**,
researching each environmental factor so that — should cutting-tool testing become possible —
the first step would be to use a phone camera at low RPM to scan the tool, preprocess the
video into event-camera-format data for wear checking, or ultimately obtain the values
directly from a phone camera or event camera in an aspirational future scenario.

### Plan A — Camera ↔ Simulation Integration

**Capture criteria:**

- One full rotation of the end mill is stitched panoramatically into a 3D surface to compute wear deviation.
- The workpiece is not imaged; its state is handled by physics prediction.

### Plan B (Optional) — Phone Camera + Event-Camera-Format Preprocessing → Simulation, or Pure Virtual Conditions

**Virtual environment:**

- Temperature, humidity
- End mill geometry, coating material
- Workpiece geometry, workpiece material
- Cooling method (cutting fluid, etc.)

**Virtual interaction:**

- Stress increase between end mill and workpiece
- Stress reduction on end mill via cooling
- Stress reduction on workpiece via cooling

**Phone-camera scenario hypothesis:**

Feasible capture conditions are around 1 000 RPM:

0. Focus on the end mill's initial position.
1. During machining, modify RPM and feed rate via G-code so the spindle decelerates at a chosen instant; capture the tool state at that moment and transmit to a laptop.
2. On the laptop, run Depth Estimation for geometry and a mask-generation model for precise segmentation; use both outputs to establish the baseline.
3. In subsequent captures, extract feature deltas from the original baseline.
   - Accumulate feature values and use a time-series model to reconcile any temporal misalignment.

> ⚠️ This hypothesis is not guaranteed to be correct and may remain purely theoretical
> due to the absence of physical cutting tools.

### Common Note

- End mill brand names cannot be used due to copyright concerns. Instead, tool geometry
  and coating material are provided as presets.

---

# End Mill Sandbox

A physics-driven CNC end mill machining simulator, tool wear analysis platform, and
cutting-condition curation engine — built in Rust with a Tauri desktop shell.

> **Spec · Coat · Cut · Measure · Decide**

---

## What This Project Does

End Mill Sandbox models the full lifecycle of an end mill in a machining operation:

| Stage | Capability |
| --- | --- |
| **Tool Selection** | Curate candidate end mills from a SQLite + LanceDB library using physics scoring, direct history, and relay evidence from similar workpieces/tools |
| **Cutting Condition Calculation** | Mechanistic force model, Kienzle power law, chip-thinning correction, Johnson-Cook flow stress |
| **Process Simulation** | Grid-based material removal along 6 toolpath patterns (pocket zigzag, rectangular profile, circular, helical, contour multi-pass, slot) with per-step force / power / temperature / wear tracking |
| **Thermal & Tribology** | Interface temperature (Boothroyd partition), coating × workpiece × coolant interaction matrix, boiling-regime heat transfer, thermal shock / crack risk, BUE risk |
| **Wear Prediction** | 4-mechanism decomposition (abrasion · adhesion · diffusion · oxidation), 3-phase trajectory (break-in → steady → accelerated), calibration against measured logs |
| **Chatter & Dynamics** | Regenerative chatter stability lobes (Altintas–Budak), forced vibration at tooth-passing frequency, process damping, variable-pitch gain, measured-FRF override with stickout scaling |
| **Tool Image Analysis** | SigLIP2 (large patch16 512) zero-shot wear-type classification, patch-vector deviation z-score, silhouette-based radial loss measurement (µm resolution) |
| **Mold / Workpiece Metrology** | Heightmap deviation fit (Z offset · radial wear · deflection · thermal · tilt decomposition), wall CMM gain calibration, mold geometry vector comparison |
| **Time-Series Forecasting** | Granite TTM-R3 (safetensors) quantile forecasting with physics-residual mode; Holt-damped fallback |
| **Decision Engine** | Deterministic safety gates (power · torque · deflection · temperature · chatter · wear · forecast · mold residual · tilt) + optional laya-typed-decisions advisor (escalation-only) |
| **Adaptive Feed G-code** | Episode-segmented load timeline → per-segment feed override → exportable NC program |
| **Statistical Ledger (SDS)** | Welford + ring-buffer hierarchical write across 4 ledgers (tool · mold · process · decision), drift detection, calibration shrinkage, confusion tracking |

---

## Architecture Overview

```text
┌─────────────────────────────────────────────────────────────┐
│  Tauri Shell (src-tauri/)                                   │
│  ├─ commands.rs   – 50+ IPC commands                        │
│  └─ main.rs       – window lifecycle, state flush           │
├─────────────────────────────────────────────────────────────┤
│  Frontend (index.html, single-file)                         │
│  ├─ 3D toolpath viewport (canvas, drag/scroll rotation)     │
│  ├─ Coolant particle visualisation                          │
│  ├─ Precision Analysis (7-tab overlay)                      │
│  └─ Library / Curation panel                                │
├─────────────────────────────────────────────────────────────┤
│  Rust Library  (src/)                                       │
│  ├─ physics.rs        – core cutting analysis               │
│  ├─ tribology.rs      – friction / coating / coolant chem   │
│  ├─ dynamics.rs       – chatter lobes, forced vibration     │
│  ├─ loadsim.rs        – grid-based path simulation          │
│  ├─ toolimage.rs      – silhouette / edge extraction        │
│  ├─ wear.rs           – SigLIP2 comparison engine           │
│  ├─ mold.rs           – heightmap fit / wall CMM            │
│  ├─ sds.rs            – statistical dynamics store          │
│  ├─ decision.rs       – gate evaluation + advisor           │
│  ├─ ingest.rs         – document parser (spec/NC/CSV/img)   │
│  ├─ pipeline.rs       – workspace orchestrator              │
│  ├─ timeseries.rs     – forecast (TTM / Holt)               │
│  ├─ gcode.rs          – G-code generation                   │
│  ├─ store/            – SQLite (rusqlite) + LanceDB         │
│  │  ├─ rdb.rs         – relational schema                   │
│  │  ├─ lance.rs       – LanceDB connection wrapper          │
│  │  ├─ vector.rs      – ANN vector collections              │
│  │  ├─ series.rs      – time-series point/window store      │
│  │  ├─ attr.rs        – attribute vector definitions        │
│  │  └─ record.rs      – run/ingest/decision recording       │
│  ├─ curation.rs       – candidate scoring & relay           │
│  ├─ ml/               – model loading                       │
│  │  ├─ siglip.rs      – SigLIP2 vision + text               │
│  │  ├─ ttm.rs         – TTM-R3 forecaster                   │
│  │  ├─ laya.rs        – laya decision advisor               │
│  │  └─ modernbert.rs  – ModernBERT encoder                  │
│  └─ profile.rs / cutting.rs / spec.rs / coating.rs / …      │
└─────────────────────────────────────────────────────────────┘

```

---

## Key Dependencies

| Crate | Role |
| --- | --- |
| `candle-core` / `candle-nn` | Tensor inference (CPU / CUDA via feature flag) |
| `tokenizers` | SigLIP2 / laya text encoding |
| `image` | PNG / JPEG / TIFF / BMP decode for tool & mold images |
| `rusqlite` (bundled) | Relational library (projects, presets, profiles, runs, metrics) |
| `lancedb` | Vector search (endmill/workpiece attributes, cut regime, time-series windows) |
| `tokio` / `futures` | Async runtime for LanceDB |
| `serde` / `serde_json` | Serialisation throughout |
| `thiserror` | Error types |
| `regex` | Tool-spec text parsing |
| `base64` | Image transfer over IPC |
| `tauri` | Desktop shell |

---

## Getting Started

### Prerequisites

* Rust ≥ 1.75 (edition 2021)
* Node.js ≥ 18 + npm
* Tauri CLI v1 (`@tauri-apps/cli ^1.5`)
* *(Optional)* CUDA toolkit + cuDNN for GPU inference → `--features cuda`
* *(Optional)* Model weights in safetensors format (SigLIP2, TTM-R3, laya)

### Build & Run

```bash
# Install frontend deps
npm install

# Development (CPU)
cargo tauri dev

# Development (CUDA)
cargo tauri dev -- --features cuda

# Release build
cargo tauri build -- --features cuda

```

> On Windows, see `run.bat` / `build.bat` for pre-configured MSVC + NVCC environment.

### Model Weights

Place safetensors model folders under a root directory (default: `<app_data>/models/`),
then specify the root in **Precision Analysis → ① Input/Model → Folder**.

Expected folder naming (keyword search):

| Keyword | Example folder | Purpose |
| --- | --- | --- |
| `siglip` | `siglip2-large-patch16-512/` | Tool & mold image embedding |
| `ttm` | `granite-timeseries-ttm-r3/52-16-dec-52-r3/` | Wear time-series forecast |
| `laya` | `laya-typed-decisions/` | Decision advisor (ModernBERT encoder) |

> **ONNX is not supported.** Only `.safetensors` / `.gguf` weights are accepted.

---

## Simulation Physics — Current Coverage

### Force Model

* Mechanistic oblique-cutting force integration over helix angle + axial slices
* Kienzle power-law specific energy with `mc` exponent
* Flank-wear land force (σ_flank · VB · db)
* Edge-force terms (Kte, Kre) with minimum-chip-thickness ploughing ramp
* Runout per-tooth offset
* Chip thinning factor for radial immersion < 50 %
* Work-hardening multiplier on specific energy

### Thermal Model

* Boothroyd–Scott partition → chip/workpiece heat split
* Loewen–Shaw convective rise with Péclet number
* Coolant film coefficient (jet velocity, boiling regime: nucleate / film / Leidenfrost)
* Coating thermal barrier (steady + transient contact resistance)
* Tool body temperature → radial/axial thermal growth (µm)
* Workpiece lumped-capacitance steady-state rise
* Thermal shock index (interrupted cutting + quench severity)

### Wear Model

* 4 mechanisms: abrasion (hard-phase indentation), adhesion (sticking × load), diffusion/ dissolution (Arrhenius, two-zone activation), oxidation (environment-dependent)
* Reference-state normalisation (AlTiN, flood, 10 mm, S45C baseline)
* 3-phase trajectory: break-in (exponential decay) → steady → accelerated (force-temperature feedback loop, 1500-step ODE)
* Coating compatibility factor, runout factor, chip-evacuation factor
* Calibration via SDS ledger (measured/predicted ratio, shrinkage estimator)

### Dynamics

* Altintas–Budak analytic stability lobes (2-branch eigenvalue solution)
* FRF: cantilever beam + holder spring; measured tap-test override with stickout scaling
* Process damping (wavelength vs. flank wear land)
* Variable pitch gain
* Robust lobe search (±2 % RPM band)
* Forced vibration: harmonic decomposition of Fx/Fy at tooth-passing harmonics → SLE

### Toolpath Load Simulation

* Voxel grid (cell ≤ D/20) with per-cell height tracking
* 6 patterns with arc interpolation, helical entry, multi-pass contour
* Per-step: engagement angle, mechanistic force, thermal, wear accumulation, wall/floor error
* Episode segmentation (k-distance on power/force/deflection/temp/engagement)
* Adaptive feed-scale per episode
* Thin-wall compliance (plate bending, thickness search)
* G00 collision & G01 LOC-exceed warnings

---

## Project Structure

```text
endmill-sandbox/
├── Cargo.toml                  # library crate (endmill_model)
├── index.html                  # Tauri frontend (single-file SPA)
├── package.json / vite.config.js
├── src/
│   ├── lib.rs                  # public module re-exports
│   ├── physics.rs              # core analysis engine
│   ├── tribology.rs            # friction · coating · coolant chemistry
│   ├── dynamics.rs             # chatter lobes · forced vibration
│   ├── loadsim.rs              # toolpath load simulation
│   ├── toolimage.rs            # image processing (silhouette, edge)
│   ├── wear.rs                 # SigLIP2 wear comparison
│   ├── mold.rs                 # heightmap / wall CMM analysis
│   ├── sds.rs                  # statistical dynamics store
│   ├── decision.rs             # safety gates + advisor
│   ├── ingest.rs               # document parser
│   ├── pipeline.rs             # workspace orchestrator
│   ├── timeseries.rs           # forecast engine
│   ├── gcode.rs                # G-code generation
│   ├── profile.rs              # presets / profiles / machine limits
│   ├── cutting.rs              # speeds & feeds calculator
│   ├── spec.rs / coating.rs / material.rs / metadata.rs / application.rs
│   ├── workpiece_setup.rs
│   ├── ml/
│   │   ├── mod.rs / siglip.rs / ttm.rs / laya.rs / modernbert.rs
│   └── store/
│       ├── mod.rs / rdb.rs / lance.rs / vector.rs / series.rs
│       ├── attr.rs / record.rs
│   └── bin/main.rs             # CLI demo
├── src-tauri/
│   ├── Cargo.toml / build.rs / tauri.conf.json
│   └── src/
│       ├── main.rs             # Tauri entry
│       └── commands.rs         # 50+ IPC commands
├── tests/
│   ├── interaction_physics.rs
│   ├── pipeline_flow.rs
│   └── store_flow.rs
├── examples/physics_probe.rs
├── run.bat / build.bat         # Windows CUDA helpers
└── .gitignore

```

---

## Configuration & Data Directories

| Path | Content |
| --- | --- |
| `<app_data>/profiles.json` | Legacy profile store (auto-migrated to SQLite) |
| `<app_data>/library/library.sqlite` | Projects, presets, profiles, end mills, workpieces, runs, metrics |
| `<app_data>/library/lancedb/` | LanceDB tables: `endmill_attr`, `workpiece_attr`, `cut_regime`, `tool_image_d*`, `mold_geom_d*`, `ts_points`, `ts_windows` |
| `<app_data>/sds/` | SDS ledgers: `tool.json`, `mold.json`, `process.json`, `decision.json`, `runs/` |
| `<app_data>/vectors/` | Saved SigLIP2 image records (`.json` + `.bin`) |
| `<app_data>/models/` | Default model root (override via `models_root.txt`) |

---

## Feature Flags

| Flag | Effect |
| --- | --- |
| `cuda` | Enables `candle-core/cuda` + `candle-nn/cuda` for GPU tensor ops |

Set `ENDMILL_DEVICE=cpu` to force CPU even when CUDA is available.

---

## License

This project is licensed under the **Apache License, Version 2.0**.