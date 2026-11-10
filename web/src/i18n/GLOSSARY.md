# Translation glossary

Keys are the Simplified Chinese text used in the code. Each language file maps those keys to
translations. Run `node scripts/i18n.mjs` to see what is missing.

Rules for every language:

- Use the terms photographers already know from mainstream photo editors in that language, but
  write our own sentences; do not copy another product's help text.
- Keep panel labels short. Slider labels sit in a column about 84 px wide, and buttons are small.
  Prefer "Temp" over "Color Temperature" only where the language's photo tools do so too.
- Keep every `{placeholder}` exactly as written, and keep product and format names as they are:
  OpenPixels, RAW, JPEG, PNG, TIFF, HEIC, ICC, LUT, .cube, sRGB, Display P3, Adobe RGB,
  ProPhoto RGB, Lensfun, CC BY-SA 3.0, MODNet, LaMa, SkyWater-Seg, Option, Alt, ⌘, Shift.
- Keyboard keys in hints (Z, J, R, W, S, G, E, P, X, U, \, Esc) stay as they are.
- Full-width punctuation in Chinese and Japanese (，。：（）“”); ASCII punctuation in English and Korean.
- Built-in look names are our own; translate their meaning, and do not use film brand names.

| 简体中文 | English | 日本語 | 繁體中文 | 한국어 |
|---|---|---|---|---|
| 白平衡 | White Balance | ホワイトバランス | 白平衡 | 화이트 밸런스 |
| 色温 / 色调 | Temp / Tint | 色温度 / 色かぶり補正 | 色溫 / 色調 | 색온도 / 색조 |
| 曝光 / 对比度 | Exposure / Contrast | 露光量 / コントラスト | 曝光度 / 對比度 | 노출 / 대비 |
| 高光 / 阴影 / 白色 / 黑色 | Highlights / Shadows / Whites / Blacks | ハイライト / シャドウ / 白レベル / 黒レベル | 亮部 / 陰影 / 白色 / 黑色 | 밝은 영역 / 어두운 영역 / 흰색 계열 / 검정 계열 |
| 自然饱和度 / 饱和度 | Vibrance / Saturation | 自然な彩度 / 彩度 | 細節飽和度 / 飽和度 | 생동감 / 채도 |
| 去雾 / 清晰度 / 纹理 | Dehaze / Clarity / Texture | かすみの除去 / 明瞭度 / テクスチャ | 去朦朧 / 清晰度 / 紋理 | 디헤이즈 / 부분 대비 / 텍스처 |
| 锐化 / 降噪 | Sharpening / Noise Reduction | シャープ / ノイズ軽減 | 銳利化 / 雜色減少 | 선명하게 하기 / 노이즈 감소 |
| 暗角 / 颗粒 | Vignette / Grain | 周辺光量補正 / 粒子 | 暈影 / 顆粒 | 비네팅 / 그레인 |
| 曲线 / 参数曲线 | Tone Curve / Parametric Curve | トーンカーブ / パラメトリックカーブ | 色調曲線 / 參數曲線 | 톤 곡선 / 매개변수 곡선 |
| 色调分离 | Color Grading | カラーグレーディング | 顏色分級 | 색 보정 |
| 校准 | Calibration | キャリブレーション | 校正 | 보정 |
| 风格滤镜 | Looks | ルック | 風格濾鏡 | 룩 |
| 智能调色 | Auto Tone | 自動補正 | 智慧調色 | 자동 톤 |
| 裁剪 | Crop | 切り抜き | 裁切 | 자르기 |
| 局部调整 / 蒙版 | Local Adjustments / Mask | 部分補正 / マスク | 局部調整 / 遮色片 | 부분 보정 / 마스크 |
| 污点修复 / 仿制图章 | Spot Healing / Clone Stamp | スポット修正 / コピースタンプ | 污點修復 / 仿製印章 | 스팟 복구 / 복제 도장 |
| 软打样 | Soft Proofing | ソフト校正 | 螢幕校樣 | 소프트 교정 |
| 星标 / 旗标 / 色标 | Rating / Flag / Color Label | レーティング / フラグ / カラーラベル | 星級 / 旗標 / 色彩標籤 | 별점 / 플래그 / 색상 레이블 |
| 选中 / 淘汰 | Pick / Reject | 採用 / 除外 | 挑選 / 退件 | 선택 / 거부 |
| 快照 / 历史 / 预设 | Snapshot / History / Preset | スナップショット / ヒストリー / プリセット | 快照 / 步驟記錄 / 預設集 | 스냅숏 / 작업 내역 / 사전 설정 |
| 照片 / 项目 | Photos / Project | 写真 / プロジェクト | 相片 / 專案 | 사진 / 프로젝트 |
| 导出 / 导入 | Export / Import | 書き出し / 読み込み | 轉存 / 匯入 | 내보내기 / 가져오기 |
