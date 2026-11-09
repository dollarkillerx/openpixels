# OpenPixels

开源、Web 优先的 AI 人像修图工具。核心用 Rust 编写，同时编译到 WASM（浏览器）和原生（桌面）。

当前阶段：早期开发。已有 RAW 解码与默认影调、WebGPU 实时调色（含 HSL、清晰度、锐化）、MODNet 人像抠图，以及图库、预设同步、批量导出。修图参数保存在浏览器本地，不修改原图；重新打开同一批文件时会自动恢复。

## 结构

| 路径 | 内容 |
|---|---|
| `crates/op-core` | 解码（RAW/JPEG/PNG）、线性图像缓冲、调色参数 |
| `crates/op-gpu` | wgpu 渲染：代理图预览、分块导出、WGSL 着色器 |
| `crates/op-wasm` | 浏览器绑定 |
| `web` | React 界面，引擎运行在 Worker + OffscreenCanvas 中 |
| `scripts/make-test-dng.mjs` | 生成任意尺寸的合成 DNG，用于测试 |

## 运行

需要 Rust（含 `wasm32-unknown-unknown` 目标）、Node、pnpm，以及支持 WebGPU 的浏览器。

```bash
scripts/build-wasm.sh
```

```bash
scripts/fetch-model.sh
```

```bash
pnpm --dir web install
```

```bash
pnpm --dir web dev
```

## 测试

```bash
cargo test -p op-core -p op-gpu
```

`op-gpu` 的测试需要本机有可用的 GPU。

## 贡献约定：第三方代码

本项目采用 Apache-2.0。

- [RapidRAW](https://github.com/CyberTimon/RapidRAW)（AGPL-3.0）只作为架构思路的参考，**不得复制或改写其代码和着色器**。
- 不得提取或复用像素蛋糕的模型、预设、素材和着色器。
- 引入模型前要分别核对代码和权重的许可证，两者经常不同。
