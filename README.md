# OpenPixels

开源、Web 优先的 AI 人像修图工具。核心用 Rust 编写，同时编译到 WASM（浏览器）和原生（桌面）。

当前阶段：早期开发。已有 RAW 解码与默认影调、相机匹配色彩、镜头校正、透视矫正、WebGPU 实时调色（含白平衡吸管、曲线、HSL、色轮、LUT 滤镜、清晰度、纹理、去雾、锐化、降噪、颗粒）、MODNet 人像抠图、基础皮肤修饰（磨皮、祛斑）、裁剪旋转、污点修复、仿制图章、LaMa AI 消除、液化、局部调整（含亮度和色彩范围），以及前后对比、星标和旗标选片、项目库、预设同步、批量导出（JPEG、8/16 位 PNG 和 TIFF，内嵌色彩配置和拍摄信息，可设尺寸、质量、文件名规则和水印）。支持 RAW、JPEG、PNG、TIFF（含 16 位）和 HEIC。修图参数保存在浏览器本地，不修改原图；重新打开同一批文件时会自动恢复。

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

## 构建与部署

先按上面的步骤构建 WASM、下载模型，再生成静态文件：

```bash
pnpm --dir web build
```

产物在 `web/dist`，约 180 MB，放到任意静态托管即可，不需要后端，照片也不会上传。部署时注意：

- 必须部署在域名根路径：模型和 ONNX Runtime 用的是 `/models/`、`/ort/` 这样的绝对路径。
- 服务器要以 `application/wasm` 返回 `.wasm`。
- `lama.onnx`（93 MB）、`modnet.onnx`（25 MB）和 ONNX Runtime 的 wasm（29 MB）体积较大，部分托管平台对单个文件有大小限制。
- 不需要配置 COOP/COEP 响应头，导出的多线程是 Worker 池，没有用 SharedArrayBuffer。

本地预览生产构建：

```bash
pnpm --dir web exec vite preview
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
- LaMa 的代码和 ONNX 导出都标注 Apache-2.0，但没有单独说明权重。商业发布前要核实其训练数据（Places2）的使用条款。
- HEIC 解码使用 [libheif-js](https://github.com/catdad-experiments/libheif-js)（LGPL-3.0），作为独立文件按需加载。HEVC 涉及专利授权，商业发布前要单独评估。
