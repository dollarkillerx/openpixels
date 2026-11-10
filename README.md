# OpenPixels

开源、Web 优先的修图工具，对标 Lightroom Classic 和像素蛋糕。核心用 Rust 编写，同时编译到 WASM（浏览器）和原生（桌面）；浏览器版用 WebGPU 实时渲染。照片只在本机处理，不上传，也不修改原图。

## 两个版本

| 版本 | 形态 | 功能 |
|---|---|---|
| Lite | 浏览器，静态部署 | 对齐 Lightroom Classic 的修图和选片功能 |
| 完整版 | 计划用 Avalonia 做桌面端；目前可用网页开发版预览 | Lite 全部功能，加上皮肤修饰、液化、换背景、换天、追色等 |

## 功能

**导入与查看**
- RAW（含 Canon CR3、Nikon NEF、Sony ARW、Fuji RAF、DNG）、JPEG、PNG、TIFF（含 16 位）、HEIC
- 适合窗口、100%、200% 和滚轮缩放，放大后显示全分辨率细节
- 前后对比（拖动分割线，或按住 `\` 看整张修图前）
- 直方图和高光、阴影溢出警告

**选片与管理**
- 星标、旗标、色标及对应筛选，网格视图
- 智能分组（按相似度和拍摄时间）和自动选片
- 项目库、历史记录、快照、复制粘贴设置、预设、同步到选中或全部

**调色**
- 白平衡和吸管，相机匹配色彩配置
- 智能调色：按直方图自动设置影调
- 曝光、对比度、高光、阴影、白色、黑色
- 自然饱和度和饱和度，去雾、清晰度、纹理
- 锐化（含半径、细节、蒙版）、明度和色彩降噪、暗角、裁剪后暗角、颗粒
- 曲线：可自由加点，背后显示直方图，另有参数曲线
- HSL、颜色混合点、色调分离色轮、校准
- 24 种内置风格滤镜（胶片、电影、创意、黑白），可导入 `.cube` LUT

**构图与镜头**
- 裁剪（固定比例或自由）、旋转、90° 转向、透视矫正
- 通过 Lensfun 镜头库对 Canon、Nikon、Fuji 等做自动镜头校正，也可手动校正畸变、暗角和色差

**修饰**
- 污点修复、仿制图章、红眼去除、LaMa AI 消除
- 局部调整：线性、径向、画笔、全图，可加亮度和色彩范围
- AI 主体、背景、天空蒙版，可添加到蒙版或从蒙版减去

**输出**
- JPEG、8/16 位 PNG 和 TIFF；色彩空间可选 sRGB、Display P3、Adobe RGB、ProPhoto RGB
- 内嵌色彩配置和拍摄信息，可设尺寸、质量、文件名规则、文字或图片水印、导出预设
- 批量导出：支持的浏览器直接写入文件夹，否则打包为 ZIP
- 软打样：导入打印机或显示器的 ICC 配置，预览打印效果，可选渲染方式、纸张和油墨模拟、色域警告

**仅完整版**：磨皮、祛斑、液化、换背景、换天、追色。

AI 功能用到的模型：主体和背景蒙版用 MODNet，天空蒙版和换天用 SkyWater-Seg，消除用 LaMa，人脸定位用 MediaPipe。

修图参数保存在浏览器本地；重新打开同一批文件时会自动恢复。

## 快捷键

| 按键 | 作用 |
|---|---|
| `0`–`5` | 星标 |
| `6`–`9` | 红、黄、绿、蓝色标 |
| `P` / `X` / `U` | 选中、淘汰、取消旗标 |
| `G` / `E` 或回车 | 网格视图、单张视图 |
| `Z` | 在适合窗口和 100% 之间切换 |
| 空格加拖动 | 放大时平移 |
| `J` | 溢出警告 |
| `\` | 按住看修图前 |
| `S` | 软打样 |
| `⌘Z` / `⇧⌘Z`（Windows 用 `Ctrl`） | 撤销、重做 |

## 结构

| 路径 | 内容 |
|---|---|
| `crates/op-core` | 解码、线性图像、调色参数、曲线、镜头和几何、修复、ICC 读写和软打样、导出编码 |
| `crates/op-gpu` | wgpu 渲染：代理图预览、分块导出、WGSL 着色器 |
| `crates/op-wasm` | 浏览器绑定 |
| `web` | React 界面，引擎运行在 Worker 和 OffscreenCanvas 中 |
| `scripts` | 构建 WASM、下载模型和镜头库、生成测试用 DNG |

## 运行

需要 Rust（含 `wasm32-unknown-unknown` 目标）、Node、pnpm，以及支持 WebGPU 的浏览器。

```bash
scripts/build-wasm.sh
```

```bash
scripts/fetch-model.sh
```

```bash
scripts/fetch-lensfun.sh
```

```bash
pnpm --dir web install
```

```bash
pnpm --dir web dev
```

开发时 `pnpm --dir web dev` 是完整版，`pnpm --dir web dev:lite` 是 Lite。

## 构建与部署

`pnpm --dir web build` 生成对外发布的 Lite，`pnpm --dir web build:full` 生成包含全部功能的版本。先按上面的步骤构建 WASM、下载模型和镜头库，再生成静态文件：

```bash
pnpm --dir web build
```

产物在 `web/dist`，约 220 MB，放到任意静态托管即可，不需要后端。部署时注意：

- 必须部署在域名根路径：模型和 ONNX Runtime 用的是 `/models/`、`/ort/` 这样的绝对路径。
- 服务器要以 `application/wasm` 返回 `.wasm`。
- `lama.onnx`（93 MB）、`skywater.onnx`（50 MB）、`modnet.onnx`（25 MB）和 ONNX Runtime 的 wasm（29 MB）体积较大，部分托管平台对单个文件有大小限制。
- 不需要配置 COOP/COEP 响应头：导出用 Worker 池并行，没有用 SharedArrayBuffer。

本地预览生产构建：

```bash
pnpm --dir web exec vite preview
```

## 测试

```bash
cargo test -p op-core -p op-gpu
```

```bash
pnpm --dir web test
```

`op-gpu` 的测试需要本机有可用的 GPU。曲线在 Rust 和 TypeScript 中各有一份实现，两边的测试都对照 `crates/op-core/src/curve_cases.json`，保证画出来的曲线和实际效果一致。

## 已知限制

- 只在 Chrome 系浏览器（含 Edge）上测试过；Safari 和 Firefox 的 WebGPU 支持还在变化。
- 调色管线在 sRGB 原色下工作。导出到 P3、Adobe RGB 或 ProPhoto 时转换和配置都正确，但不会保留超出 sRGB 的颜色。
- 网页无法读取系统的 ICC 文件，软打样需要手动导入配置。
- AI 消除单线程运行，一次约 10–20 秒。

## 贡献约定：第三方代码

本项目采用 Apache-2.0。

- [RapidRAW](https://github.com/CyberTimon/RapidRAW)（AGPL-3.0）只作为架构思路的参考，**不得复制或改写其代码和着色器**。
- 不得提取或复用像素蛋糕的模型、预设、素材和着色器。
- 内置风格滤镜按公开的影调特征自行调制，不是任何厂商的官方配方，名称也不使用胶片厂商的商标。
- 引入模型前要分别核对代码和权重的许可证，两者经常不同。
- LaMa 的代码和 ONNX 导出都标注 Apache-2.0，但没有单独说明权重。商业发布前要核实其训练数据（Places2）的使用条款。
- Canon、Nikon、Fuji 等 RAW 的自动镜头校正用 [Lensfun](https://github.com/lensfun/lensfun) 数据库（CC BY-SA 3.0）。只用数据，不用其 LGPL 代码；转换后的 `web/public/lensfun/lensfun.json` 要按同样的许可证分发并署名，说明在同目录的 `NOTICE.txt`。
- 换天用 [SkyWater-Seg](https://huggingface.co/Realcat/skywater_seg)，仓库标注 MIT，但它用 ADE20K 训练，商业发布前要核实该数据集的使用条款。
- HEIC 解码使用 [libheif-js](https://github.com/catdad-experiments/libheif-js)（LGPL-3.0），作为独立文件按需加载。HEVC 涉及专利授权，商业发布前要单独评估。
- 测试素材放在不进版本库的 `web/public/fixtures/`，其中的系统 ICC 配置和网上下载的照片不随项目分发。
