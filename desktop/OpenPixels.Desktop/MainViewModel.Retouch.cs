using System.Collections.ObjectModel;
using System.Text.Json.Nodes;
using Avalonia;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

public enum RetouchTool { None, Spot, Clone, RedEye, Erase, Linear, Radial, Brush, RefineAdd, RefineSubtract, Liquify }

public sealed partial class LayerRow(int index, string title) : ObservableObject
{
    public int Index { get; } = index;
    public string Title { get; } = title;

    [ObservableProperty]
    private bool _isActive;
}

/// <summary>What the stage overlay draws for the tools, in device pixels.</summary>
public sealed record ToolOverlayState(
    IReadOnlyList<(Point A, Point B, bool Radial, bool Active)> Handles,
    IReadOnlyList<(Point Centre, double Radius)> Dabs,
    Point? StampSource,
    double BrushRadius);

/// <summary>Healing, clone stamp, red-eye, AI erase, and local adjustments with AI masks.</summary>
public sealed partial class MainViewModel
{
    private const double StampSpacing = 0.35;
    private const double EraseSpacing = 0.3;
    /// <summary>How near, in device pixels, a press must be to grab a gradient handle.</summary>
    private const double HandleGrab = 16;
    private const double NewLayerExposure = 0.5;

    private readonly RetouchStore _retouch = new();
    private readonly Lazy<AiModels> _ai = new(() => new AiModels());
    /// <summary>Model masks per photo, at the main proxy's size, so they are found once.</summary>
    private readonly Dictionary<string, Dictionary<string, ModelMask>> _modelMasks = [];
    private List<LocalLayer> _locals = [];
    private JsonArray _heals = [];
    private List<ErasePatch> _erases = [];
    private readonly List<EraseDab> _eraseDabs = [];
    private (int Width, int Height) _proxySize = (1, 1);
    private bool _maskQueued;
    private bool _maskSending;

    // A drag in progress.
    private UvPoint? _stampSource;
    private UvPoint? _stampOffset;
    private UvPoint? _lastPoint;
    private long _stampGroup;
    private bool _drawing;
    private (int Layer, bool End)? _handleDrag;

    public ObservableCollection<LayerRow> Layers { get; } = [];
    public string[] RangeModes { get; } = [I18n.T("无"), I18n.T("亮度范围"), I18n.T("颜色范围")];

    [ObservableProperty]
    private RetouchTool _tool;

    [ObservableProperty]
    private double _healRadius = 0.012;

    [ObservableProperty]
    private double _localBrushRadius = 0.04;

    [ObservableProperty]
    private int _activeLayer = -1;

    [ObservableProperty]
    private bool _showMask;

    [ObservableProperty]
    private bool _aiBusy;

    [ObservableProperty]
    private string _retouchStatus = "";

    public bool IsSpot => Tool == RetouchTool.Spot;
    public bool IsClone => Tool == RetouchTool.Clone;
    public bool IsRedEye => Tool == RetouchTool.RedEye;
    public bool IsErase => Tool == RetouchTool.Erase;
    public bool IsLinear => Tool == RetouchTool.Linear;
    public bool IsRadial => Tool == RetouchTool.Radial;
    public bool IsBrush => Tool == RetouchTool.Brush;
    public bool IsRefineAdd => Tool == RetouchTool.RefineAdd;
    public bool IsRefineSubtract => Tool == RetouchTool.RefineSubtract;
    public bool UsesHealBrush => Tool is RetouchTool.Spot or RetouchTool.Clone or RetouchTool.RedEye or RetouchTool.Erase;
    public bool HasHeals => _heals.Count > 0;
    public bool HasErases => _erases.Count > 0;
    public bool CanRunErase => _eraseDabs.Count > 0 && !AiBusy && AiModels.Has(AiModels.Inpaint.File);
    public string HealSummary => _heals.Count == 0 && _erases.Count == 0 ? ""
        : _erases.Count == 0 ? I18n.T("已修复 {heals} 处", ("heals", _heals.Count))
        : I18n.T("已修复 {heals} 处，AI 消除 {erases} 处", ("heals", _heals.Count), ("erases", _erases.Count));
    public bool AiAvailable => AiModels.Has(AiModels.Matte.File);
    public bool SkyAvailable => AiModels.Has(AiModels.Sky.File);
    public bool CanAddLayer => CanEdit && _locals.Count < LocalMasks.MaxLayers && !AiBusy;
    public bool HasActiveLayer => ActiveLayer >= 0 && ActiveLayer < _locals.Count;
    public bool ActiveIsBrushable => HasActiveLayer && _locals[ActiveLayer].Kind is LocalKind.Brush or LocalKind.Subject or LocalKind.Background or LocalKind.Sky;
    public IReadOnlyList<SliderItem> ActiveLocalSliders => HasActiveLayer ? LocalMasks.Sliders.Select(slider => Slider($"local{ActiveLayer}{slider.Suffix}")).ToList() : [];
    public IReadOnlyList<SliderItem> ActiveRangeSliders => !HasActiveLayer ? [] : ActiveRangeMode switch
    {
        1 => [Slider($"local{ActiveLayer}RangeLow"), Slider($"local{ActiveLayer}RangeHigh")],
        2 => [Slider($"local{ActiveLayer}RangeHue"), Slider($"local{ActiveLayer}RangeWidth")],
        _ => [],
    };

    public int ActiveRangeMode
    {
        get => HasActiveLayer ? (int)_edit.GetValueOrDefault($"local{ActiveLayer}RangeMode") : 0;
        set
        {
            if (!HasActiveLayer || _showingEdit || value == ActiveRangeMode || value < 0) return;
            ApplyEdit(With(($"local{ActiveLayer}RangeMode", value, 0)), I18n.T("局部范围"));
        }
    }

    public bool ActiveInvert
    {
        get => HasActiveLayer && _locals[ActiveLayer].Invert;
        set
        {
            if (!HasActiveLayer || value == ActiveInvert) return;
            ChangeLayers(layers => layers[ActiveLayer] = layers[ActiveLayer] with { Invert = value }, I18n.T("反相局部图层"));
        }
    }

    /// <summary>The sliders for each local layer, made once so ShowEdit keeps them in step.</summary>
    private void MakeLocalSliders()
    {
        for (var layer = 0; layer < LocalMasks.MaxLayers; layer++)
        {
            foreach (var (suffix, label, min, max) in LocalMasks.Sliders) Make(new SliderSpec($"local{layer}{suffix}", label, min, max));
            Make(new SliderSpec($"local{layer}RangeLow", I18n.T("最暗"), 0, 1));
            Make(new SliderSpec($"local{layer}RangeHigh", I18n.T("最亮"), 0, 1, 1));
            Make(new SliderSpec($"local{layer}RangeHue", I18n.T("色相"), 0, 360));
            Make(new SliderSpec($"local{layer}RangeWidth", I18n.T("色相范围"), 5, 90, 30));
        }
    }

    partial void OnToolChanged(RetouchTool value)
    {
        if (value != RetouchTool.None)
        {
            Picker = Picker.None;
            CropEditing = false;
        }
        _drawing = false;
        foreach (var name in new[] { nameof(IsSpot), nameof(IsClone), nameof(IsRedEye), nameof(IsErase), nameof(IsLinear), nameof(IsRadial), nameof(IsBrush), nameof(IsRefineAdd), nameof(IsRefineSubtract), nameof(UsesHealBrush), nameof(IsLiquify) })
        {
            OnPropertyChanged(name);
        }
        ToolHint = value switch
        {
            RetouchTool.Spot => I18n.T("点击要去除的斑点或杂物"),
            RetouchTool.Clone => _stampSource is null ? I18n.T("按住 Option（Alt）点击取样，再在别处涂抹") : I18n.T("涂抹以复制取样处"),
            RetouchTool.RedEye => I18n.T("点击红眼的瞳孔"),
            RetouchTool.Erase => I18n.T("涂抹要消除的物体，然后点“消除”"),
            RetouchTool.Linear => I18n.T("在画面上拖出渐变方向"),
            RetouchTool.Radial => I18n.T("从中心向外拖出范围"),
            RetouchTool.Brush => I18n.T("在要调整的地方涂抹"),
            RetouchTool.RefineAdd => I18n.T("涂抹以加入蒙版"),
            RetouchTool.RefineSubtract => I18n.T("涂抹以从蒙版中去掉"),
            RetouchTool.Liquify => I18n.T("在画面上拖动以变形"),
            _ => ToolHint,
        };
        if (value == RetouchTool.None && Picker == Picker.None) ToolHint = "";
        OverlayChanged?.Invoke();
    }

    partial void OnActiveLayerChanged(int value)
    {
        foreach (var row in Layers) row.IsActive = row.Index == value;
        RefreshLayerProperties();
        if (ShowMask) SendOverlayFlag();
        OverlayChanged?.Invoke();
    }

    partial void OnShowMaskChanged(bool value) => SendOverlayFlag();

    partial void OnAiBusyChanged(bool value) => RefreshLayerProperties();

    private void SendOverlayFlag()
    {
        _editChanged = true;
        RequestRender();
    }

    /// <summary>The layer whose mask is tinted on screen, 1-based, or 0.</summary>
    private int LocalOverlay => ShowMask && HasActiveLayer ? ActiveLayer + 1 : 0;

    private void RefreshLayerProperties()
    {
        foreach (var name in new[]
        {
            nameof(HasActiveLayer), nameof(ActiveIsBrushable), nameof(ActiveLocalSliders), nameof(ActiveRangeSliders), nameof(ActiveRangeMode),
            nameof(ActiveInvert), nameof(CanAddLayer), nameof(CanRunErase), nameof(HasHeals), nameof(HasErases), nameof(HealSummary),
        })
        {
            OnPropertyChanged(name);
        }
    }

    private void SyncLayerRows()
    {
        Layers.Clear();
        for (var index = 0; index < _locals.Count; index++) Layers.Add(new LayerRow(index, $"{index + 1}. {_locals[index].Title}") { IsActive = index == ActiveLayer });
        if (ActiveLayer >= _locals.Count) ActiveLayer = _locals.Count - 1;
        RefreshLayerProperties();
    }

    // ---- Opening ----

    /// <summary>Loads this photo's heals, erases and local layers into the main engine.</summary>
    private async Task LoadRetouchAsync(PhotoItem photo, JsonNode proxy)
    {
        _proxySize = ((int)proxy["width"]!, (int)proxy["height"]!);
        RetouchStatus = "";
        _heals = _retouch.Heals(photo.Id);
        _erases = _retouch.Erases(photo.Id);
        _eraseDabs.Clear();
        _stampSource = _stampOffset = null;
        _locals = _history?.CurrentLocals ?? _retouch.Locals(photo.Id);
        _warps = _retouch.Warps(photo.Id);
        _liquifyDrag = null;
        await ApplyRetouchAsync(_engine, photo.Id, _locals, _heals, _erases, _proxySize, main: true, _warps, _edit);
        ActiveLayer = _locals.Count > 0 ? 0 : -1;
        SyncLayerRows();
    }

    /// <summary>Sends heals, erases and local masks to an engine with an open photo.</summary>
    private async Task ApplyRetouchAsync(EngineClient engine, string id, IReadOnlyList<LocalLayer> locals, JsonArray heals, IReadOnlyList<ErasePatch> erases,
        (int Width, int Height) proxy, bool main, IReadOnlyList<JsonArray> warps, IReadOnlyDictionary<string, double> edit)
    {
        await engine.CallRawAsync("setWarpStrokes", new JsonArray(warps.SelectMany(group => group).Select(stroke => stroke!.DeepClone()).ToArray()).ToJsonString());
        await ApplyReplaceAsync(engine, id, edit, proxy);
        await engine.CallAsync("clearErases");
        foreach (var patch in erases) await PushEraseAsync(engine, patch);
        // set_heals also re-applies the erases.
        await engine.CallRawAsync("setHeals", heals.ToJsonString());
        if (locals.Count == 0)
        {
            await engine.CallAsync("setLocalMask", new { width = 1, height = 1 }, new byte[4]);
            return;
        }
        var masks = await ModelMasksAsync(engine, id, locals, proxy, main);
        var rgba = await Task.Run(() => LocalMasks.Render(locals, proxy.Width, proxy.Height, masks));
        await engine.CallAsync("setLocalMask", new { width = proxy.Width, height = proxy.Height }, rgba);
    }

    private static Task<EngineReply> PushEraseAsync(EngineClient engine, ErasePatch patch)
    {
        var data = new byte[patch.Rgb.Length + patch.Alpha.Length];
        patch.Rgb.CopyTo(data, 0);
        patch.Alpha.CopyTo(data, patch.Rgb.Length);
        return engine.CallAsync("pushErase", new { x = patch.X, y = patch.Y, width = patch.Width, height = patch.Height, size = patch.Size }, data);
    }

    /// <summary>The model masks the AI layers need, found with the given engine if not known yet.</summary>
    private async Task<Dictionary<string, ModelMask>> ModelMasksAsync(EngineClient engine, string id, IReadOnlyList<LocalLayer> locals, (int Width, int Height) proxy, bool main)
    {
        if (!_modelMasks.TryGetValue(id, out var known)) _modelMasks[id] = known = [];
        foreach (var model in locals.Where(layer => layer.IsAi).Select(layer => layer.Model).Distinct())
        {
            if (!known.ContainsKey(model)) await FindModelMaskAsync(engine, id, model, proxy);
        }
        return known;
    }

    /// <summary>
    /// Runs a segmentation model on the engine's open photo, and keeps both the raw result
    /// (for replacement, which the engine refines itself) and one refined to its edges (for local layers).
    /// </summary>
    private async Task<ModelMask> FindModelMaskAsync(EngineClient engine, string id, string model, (int Width, int Height) proxy)
    {
        byte[] mask;
        int width, height;
        if (model == "sky")
        {
            (width, height) = (AiModels.Sky.Size, AiModels.Sky.Size);
            var input = await engine.CallAsync("modelInput", new { width, height, mean = AiModels.Sky.Mean, std = AiModels.Sky.Std });
            var tensor = Floats(input.Data);
            mask = await Task.Run(() => _ai.Value.SkyMask(tensor));
        }
        else
        {
            (width, height) = AiModels.Matte.InputSize(proxy.Width, proxy.Height);
            var input = await engine.CallAsync("modelInput", new { width, height, mean = AiModels.Matte.Mean, std = AiModels.Matte.Std });
            var tensor = Floats(input.Data);
            var (w, h) = (width, height);
            mask = await Task.Run(() => _ai.Value.SubjectMatte(tensor, w, h));
        }
        var refined = await engine.CallAsync("refineModelMask", new { width, height }, mask);
        if (!_rawMasks.TryGetValue(id, out var raws)) _rawMasks[id] = raws = [];
        raws[model] = new ModelMask(mask, width, height);
        if (!_modelMasks.TryGetValue(id, out var refinedMasks)) _modelMasks[id] = refinedMasks = [];
        return refinedMasks[model] = new ModelMask(refined.Data, proxy.Width, proxy.Height);
    }

    private static float[] Floats(byte[] bytes)
    {
        var values = new float[bytes.Length / 4];
        Buffer.BlockCopy(bytes, 0, values, 0, values.Length * 4);
        return values;
    }

    // ---- Tools on the stage ----

    [RelayCommand]
    private void ChooseTool(string name)
    {
        var tool = Enum.Parse<RetouchTool>(name);
        Tool = Tool == tool ? RetouchTool.None : tool;
    }

    private async Task<UvPoint?> SourceAtAsync(double px, double py)
    {
        var reply = await _engine.CallAsync("sourceAt", new { x = px / _viewport.Width, y = py / _viewport.Height, stageWidth = _viewport.Width, stageHeight = _viewport.Height });
        return reply.Json is JsonArray { Count: 2 } uv ? new UvPoint(uv[0]!.GetValue<double>(), uv[1]!.GetValue<double>()) : null;
    }

    private (double Width, double Height) ImageSize => (_info?["width"]?.GetValue<double>() ?? 1, _info?["height"]?.GetValue<double>() ?? 1);

    /// <summary>A distance in UV turned into fractions of the long edge, the unit radii use.</summary>
    private (double X, double Y) ToLongEdge(double du, double dv)
    {
        var (width, height) = ImageSize;
        var longEdge = Math.Max(width, height);
        return (du * width / longEdge, dv * height / longEdge);
    }

    /// <summary>Where source UV appears on the stage, in device pixels, ignoring lens correction.</summary>
    public Point StageOf(UvPoint uv)
    {
        var (width, height) = ImageSize;
        var crop = CropEditing ? Crop.Full : Frame.CropOf(_edit);
        var shown = CropEditing ? new Dictionary<string, double>(_edit) { ["cropX"] = 0, ["cropY"] = 0, ["cropW"] = 1, ["cropH"] = 1 } : _edit;
        var output = Frame.SourceToOutput(uv.X, uv.Y, shown, width, height);
        var box = DisplayBox();
        _ = crop;
        return new Point(box.Left + output.X * box.Width, box.Top + output.Y * box.Height);
    }

    /// <summary>Pixels on screen per pixel of the image, for drawing brush sizes.</summary>
    private double ScreenScale()
    {
        var (width, height) = ImageSize;
        var (frameWidth, _) = Frame.FrameSize(width, height, (int)_edit.GetValueOrDefault("quarter"));
        return DisplayBox().Width / (frameWidth * (CropEditing ? 1 : Frame.CropOf(_edit).W));
    }

    public ToolOverlayState OverlayState()
    {
        var (width, height) = ImageSize;
        var longEdge = Math.Max(width, height);
        var scale = ScreenScale();
        var handles = new List<(Point, Point, bool, bool)>();
        if (_imageOpen && Tool is RetouchTool.None or RetouchTool.Linear or RetouchTool.Radial)
        {
            for (var index = 0; index < _locals.Count; index++)
            {
                var layer = _locals[index];
                if (layer.HasHandles && index == ActiveLayer) handles.Add((StageOf(layer.A), StageOf(layer.B), layer.Kind == LocalKind.Radial, true));
            }
        }
        var dabs = Tool == RetouchTool.Erase ? _eraseDabs.Select(dab => (StageOf(new UvPoint(dab.X, dab.Y)), dab.Radius * longEdge * scale)).ToList() : [];
        var radius = Tool switch
        {
            RetouchTool.Spot or RetouchTool.Clone or RetouchTool.RedEye or RetouchTool.Erase => HealRadius * longEdge * scale,
            RetouchTool.Brush or RetouchTool.RefineAdd or RetouchTool.RefineSubtract => LocalBrushRadius * longEdge * scale,
            RetouchTool.Liquify => LiquifyRadius * longEdge * scale,
            _ => 0,
        };
        Point? source = Tool == RetouchTool.Clone && _stampSource is { } stamp ? StageOf(stamp) : null;
        return new ToolOverlayState(handles, dabs, source, radius);
    }

    partial void OnHealRadiusChanged(double value) => OverlayChanged?.Invoke();
    partial void OnLocalBrushRadiusChanged(double value) => OverlayChanged?.Invoke();

    /// <summary>A press on the stage with a tool active, or on a gradient handle. Returns whether it was used.</summary>
    public async Task<bool> ToolPressAsync(double px, double py, bool alt)
    {
        if (!_imageOpen) return false;
        if (Tool == RetouchTool.None)
        {
            return GrabHandle(px, py);
        }
        if (Tool == RetouchTool.Liquify)
        {
            LiquifyPress(px, py);
            return true;
        }
        var point = await SourceAtAsync(px, py);
        if (point is not { } uv) return true;
        switch (Tool)
        {
            case RetouchTool.Spot:
                await HealSpotAsync(px, py);
                break;
            case RetouchTool.RedEye:
                await AddHealsAsync([new JsonObject { ["x"] = uv.X, ["y"] = uv.Y, ["radius"] = HealRadius, ["sourceDx"] = 0, ["sourceDy"] = 0, ["redEye"] = true }]);
                break;
            case RetouchTool.Clone when alt:
                _stampSource = uv;
                _stampOffset = null;
                ToolHint = I18n.T("涂抹以复制取样处");
                OverlayChanged?.Invoke();
                break;
            case RetouchTool.Clone:
                if (_stampSource is not { } source)
                {
                    ToolHint = I18n.T("先按住 Option（Alt）点击要取样的位置");
                    break;
                }
                // The offset stays until a new sample point is set, so strokes line up.
                _stampOffset ??= new UvPoint(source.X - uv.X, source.Y - uv.Y);
                _stampGroup = Environment.TickCount64;
                _lastPoint = null;
                _drawing = true;
                await StampAlongAsync(uv);
                break;
            case RetouchTool.Erase:
                _lastPoint = null;
                _drawing = true;
                PaintErase(uv);
                break;
            case RetouchTool.Linear or RetouchTool.Radial:
                if (_locals.Count >= LocalMasks.MaxLayers)
                {
                    RetouchStatus = I18n.T("最多 {max} 个局部图层", ("max", LocalMasks.MaxLayers));
                    break;
                }
                AddLayer(new LocalLayer { Kind = Tool == RetouchTool.Linear ? LocalKind.Linear : LocalKind.Radial, A = uv, B = uv, Radius = LocalBrushRadius });
                _handleDrag = (ActiveLayer, true);
                _drawing = true;
                break;
            case RetouchTool.Brush:
                if (!(HasActiveLayer && _locals[ActiveLayer].Kind == LocalKind.Brush))
                {
                    if (_locals.Count >= LocalMasks.MaxLayers)
                    {
                        RetouchStatus = I18n.T("最多 {max} 个局部图层", ("max", LocalMasks.MaxLayers));
                        break;
                    }
                    AddLayer(new LocalLayer { Kind = LocalKind.Brush, Radius = LocalBrushRadius });
                }
                StartStroke(uv, erase: false);
                break;
            case RetouchTool.RefineAdd or RetouchTool.RefineSubtract:
                if (!ActiveIsBrushable)
                {
                    RetouchStatus = I18n.T("先选中一个画笔或 AI 图层");
                    break;
                }
                StartStroke(uv, erase: Tool == RetouchTool.RefineSubtract);
                break;
        }
        return true;
    }

    public async Task ToolMoveAsync(double px, double py)
    {
        if (!_drawing && _handleDrag is null) return;
        if (Tool == RetouchTool.Liquify)
        {
            await LiquifyMoveAsync(px, py);
            return;
        }
        var point = await SourceAtAsync(px, py);
        if (point is not { } uv) return;
        if (_handleDrag is { } drag)
        {
            if (drag.Layer >= _locals.Count) return;
            _locals[drag.Layer] = drag.End ? _locals[drag.Layer] with { B = uv } : _locals[drag.Layer] with { A = uv };
            QueueLocalMask();
            OverlayChanged?.Invoke();
            return;
        }
        switch (Tool)
        {
            case RetouchTool.Clone:
                await StampAlongAsync(uv);
                break;
            case RetouchTool.Erase:
                PaintErase(uv);
                break;
            case RetouchTool.Brush or RetouchTool.RefineAdd or RetouchTool.RefineSubtract:
                ContinueStroke(uv);
                break;
        }
    }

    public void ToolRelease()
    {
        if (_handleDrag is not null)
        {
            _handleDrag = null;
            _drawing = false;
            RecordLayers(Tool == RetouchTool.None ? I18n.T("移动局部图层") : I18n.T("添加{title}", ("title", _locals[ActiveLayer].Title)));
            if (Tool is RetouchTool.Linear or RetouchTool.Radial) Tool = RetouchTool.None;
            return;
        }
        if (!_drawing) return;
        _drawing = false;
        if (Tool == RetouchTool.Liquify)
        {
            LiquifyRelease();
            return;
        }
        switch (Tool)
        {
            case RetouchTool.Clone:
                SaveHeals();
                break;
            case RetouchTool.Brush or RetouchTool.RefineAdd or RetouchTool.RefineSubtract:
                RecordLayers(Tool == RetouchTool.RefineSubtract ? I18n.T("擦除蒙版") : I18n.T("画笔"));
                break;
        }
    }

    private bool GrabHandle(double px, double py)
    {
        if (!HasActiveLayer || !_locals[ActiveLayer].HasHandles) return false;
        var layer = _locals[ActiveLayer];
        double Distance(UvPoint uv)
        {
            var at = StageOf(uv);
            return Math.Sqrt(Math.Pow(at.X - px, 2) + Math.Pow(at.Y - py, 2));
        }
        var grab = HandleGrab * _scaling;
        if (Distance(layer.B) <= grab) _handleDrag = (ActiveLayer, true);
        else if (Distance(layer.A) <= grab) _handleDrag = (ActiveLayer, false);
        return _handleDrag is not null;
    }

    // ---- Healing ----

    private async Task HealSpotAsync(double px, double py)
    {
        var args = new { x = px / _viewport.Width, y = py / _viewport.Height, stageWidth = _viewport.Width, stageHeight = _viewport.Height, radius = HealRadius };
        var spot = (await _engine.CallAsync("healAt", args)).Json;
        if (spot is not JsonObject found)
        {
            ToolHint = I18n.T("这里找不到合适的取样，请换个位置");
            return;
        }
        _heals.Add(found.DeepClone());
        SaveHeals();
        RequestRender();
    }

    private async Task AddHealsAsync(IReadOnlyList<JsonObject> spots)
    {
        foreach (var spot in spots) _heals.Add(spot.DeepClone());
        await _engine.CallRawAsync("addHeals", new JsonArray(spots.Select(spot => (JsonNode)spot.DeepClone()).ToArray()).ToJsonString());
        if (!_drawing) SaveHeals();
        RequestRender();
    }

    private async Task StampAlongAsync(UvPoint point)
    {
        if (_stampOffset is not { } offset) return;
        var previous = _lastPoint ?? point;
        var (travelX, travelY) = ToLongEdge(point.X - previous.X, point.Y - previous.Y);
        var travel = Math.Sqrt(travelX * travelX + travelY * travelY);
        var spacing = HealRadius * StampSpacing;
        if (_lastPoint is not null && travel < spacing) return;
        var steps = Math.Max(1, (int)Math.Floor(travel / spacing));
        var (sourceDx, sourceDy) = ToLongEdge(offset.X, offset.Y);
        var spots = new List<JsonObject>();
        for (var step = 1; step <= steps; step++)
        {
            var at = _lastPoint is null ? 1 : step / (double)steps;
            spots.Add(new JsonObject
            {
                ["x"] = previous.X + (point.X - previous.X) * at, ["y"] = previous.Y + (point.Y - previous.Y) * at, ["radius"] = HealRadius,
                ["sourceDx"] = sourceDx, ["sourceDy"] = sourceDy, ["clone"] = true, ["group"] = _stampGroup,
            });
        }
        _lastPoint = point;
        _stampSource = new UvPoint(point.X + offset.X, point.Y + offset.Y);
        await AddHealsAsync(spots);
        OverlayChanged?.Invoke();
    }

    private void SaveHeals()
    {
        if (SelectedPhoto is null) return;
        _retouch.SaveHeals(SelectedPhoto.Id, _heals);
        RefreshLayerProperties();
        _thumbnailTimer.Stop();
        _thumbnailTimer.Start();
        _settleTimer.Stop();
        _settleTimer.Start();
    }

    /// <summary>Removes the last spot, or the whole of the last clone drag.</summary>
    [RelayCommand]
    private async Task UndoHeal()
    {
        if (_heals.Count == 0) return;
        var group = _heals[^1]?["group"]?.GetValue<long>();
        var kept = _heals.Where(spot => group is null ? spot != _heals[^1] : spot?["group"]?.GetValue<long>() != group).Select(spot => spot!.DeepClone()).ToArray();
        await ReplaceHealsAsync(new JsonArray(kept));
    }

    [RelayCommand]
    private Task ClearHeals() => ReplaceHealsAsync([]);

    private async Task ReplaceHealsAsync(JsonArray heals)
    {
        _heals = heals;
        await _engine.CallRawAsync("setHeals", heals.ToJsonString());
        SaveHeals();
        RequestRender();
    }

    // ---- AI erase ----

    private void PaintErase(UvPoint point)
    {
        var previous = _lastPoint ?? point;
        var (travelX, travelY) = ToLongEdge(point.X - previous.X, point.Y - previous.Y);
        var travel = Math.Sqrt(travelX * travelX + travelY * travelY);
        var spacing = HealRadius * EraseSpacing;
        if (_lastPoint is not null && travel < spacing) return;
        var steps = Math.Max(1, (int)Math.Floor(travel / spacing));
        for (var step = 1; step <= steps; step++)
        {
            var at = step / (double)steps;
            _eraseDabs.Add(new EraseDab(previous.X + (point.X - previous.X) * at, previous.Y + (point.Y - previous.Y) * at, HealRadius));
        }
        _lastPoint = point;
        OnPropertyChanged(nameof(CanRunErase));
        OverlayChanged?.Invoke();
    }

    [RelayCommand]
    private void ClearEraseMarks()
    {
        _eraseDabs.Clear();
        OnPropertyChanged(nameof(CanRunErase));
        OverlayChanged?.Invoke();
    }

    /// <summary>Fills the painted area with LaMa and keeps the result as a patch of this photo.</summary>
    [RelayCommand]
    public async Task RunErase()
    {
        if (!CanRunErase || SelectedPhoto is not { } photo) return;
        var ticket = _openTicket;
        var (width, height) = ((int)ImageSize.Width, (int)ImageSize.Height);
        var dabs = _eraseDabs.ToList();
        AiBusy = true;
        RetouchStatus = I18n.T("AI 消除中…");
        try
        {
            var started = Environment.TickCount64;
            var crop = Erasing.Crop(dabs, width, height);
            var (mask, alpha) = Erasing.Rasterize(dabs, crop, width, height);
            var region = (X: crop.X / (double)width, Y: crop.Y / (double)height, Width: crop.Width / (double)width, Height: crop.Height / (double)height);
            var input = await _engine.CallAsync("inpaintInput", new { x = region.X, y = region.Y, width = region.Width, height = region.Height, size = Erasing.ModelSize });
            var image = Floats(input.Data);
            var rgb = await Task.Run(() => _ai.Value.Fill(image, mask));
            if (ticket != _openTicket) return;
            var patch = new ErasePatch(region.X, region.Y, region.Width, region.Height, Erasing.ModelSize, rgb, alpha);
            _erases.Add(patch);
            _retouch.SaveErases(photo.Id, _erases);
            await PushEraseAsync(_engine, patch);
            await _engine.CallAsync("refreshErases");
            _eraseDabs.Clear();
            RetouchStatus = I18n.T("已消除（{seconds} 秒）", ("seconds", ((Environment.TickCount64 - started) / 1000.0).ToString("0.0")));
            RequestRender();
            SaveHeals();
        }
        catch (Exception exception)
        {
            RetouchStatus = I18n.T("消除失败：{exception}", ("exception", Messages.Friendly(exception)));
            StatusDetail = Messages.Detail(exception);
        }
        finally
        {
            AiBusy = false;
            OverlayChanged?.Invoke();
        }
    }

    [RelayCommand]
    private Task UndoErase() => ReplaceErasesAsync(_erases.Take(Math.Max(0, _erases.Count - 1)).ToList());

    [RelayCommand]
    private Task ClearErases() => ReplaceErasesAsync([]);

    private async Task ReplaceErasesAsync(List<ErasePatch> erases)
    {
        if (SelectedPhoto is null) return;
        _erases = erases;
        _retouch.SaveErases(SelectedPhoto.Id, erases);
        await _engine.CallAsync("clearErases");
        foreach (var patch in erases) await PushEraseAsync(_engine, patch);
        await _engine.CallAsync("refreshErases");
        SaveHeals();
        RequestRender();
    }

    // ---- Local layers ----

    private void AddLayer(LocalLayer layer)
    {
        _locals.Add(layer);
        var index = _locals.Count - 1;
        // Starts visible, so the area can be judged straight away.
        _edit = With(($"local{index}Exposure", NewLayerExposure, 0));
        ActiveLayer = index;
        SyncLayerRows();
        ShowEdit();
        _editChanged = true;
        QueueLocalMask();
    }

    /// <summary>Changes the layer list, records it, and redraws the masks.</summary>
    private void ChangeLayers(Action<List<LocalLayer>> change, string label, Dictionary<string, double>? edit = null)
    {
        change(_locals);
        if (edit is not null) _edit = edit;
        SyncLayerRows();
        RecordLayers(label);
        QueueLocalMask();
    }

    /// <summary>Records the layers and the edit as one history step, and saves both.</summary>
    private void RecordLayers(string label)
    {
        if (_history is null || SelectedPhoto is null) return;
        var before = _history.Current;
        _history.Record(before, _edit, label, _locals, shapeChanged: true);
        _retouch.SaveLocals(SelectedPhoto.Id, _locals);
        EditChanged(before);
        QueueLocalMask();
    }

    [RelayCommand]
    private void AddFullLayer()
    {
        if (!CanAddLayer) return;
        AddLayer(new LocalLayer { Kind = LocalKind.Full, A = new(0.5, 0.5), B = new(0.5, 0.5), Radius = LocalBrushRadius });
        _edit = With(($"local{ActiveLayer}RangeMode", 1, 0));
        RecordLayers(I18n.T("添加全图"));
    }

    [RelayCommand]
    private async Task AddAiLayer(string kind)
    {
        if (!CanAddLayer || SelectedPhoto is not { } photo) return;
        var layer = new LocalLayer { Kind = Enum.Parse<LocalKind>(kind), A = new(0.5, 0.5), B = new(0.5, 0.5), Radius = LocalBrushRadius };
        var ticket = _openTicket;
        AiBusy = true;
        RetouchStatus = I18n.T("正在识别{title}…", ("title", layer.Title));
        try
        {
            var started = Environment.TickCount64;
            await ModelMasksAsync(_engine, photo.Id, [layer], _proxySize, main: true);
            if (ticket != _openTicket) return;
            AddLayer(layer);
            RecordLayers(I18n.T("添加{title}", ("title", layer.Title)));
            RetouchStatus = I18n.T("已识别{title}（{seconds} 秒）", ("title", layer.Title), ("seconds", ((Environment.TickCount64 - started) / 1000.0).ToString("0.0")));
        }
        catch (Exception exception)
        {
            RetouchStatus = I18n.T("识别失败：{exception}", ("exception", Messages.Friendly(exception)));
            StatusDetail = Messages.Detail(exception);
        }
        finally
        {
            AiBusy = false;
        }
    }

    [RelayCommand]
    private void SelectLayer(LayerRow row) => ActiveLayer = row.Index;

    [RelayCommand]
    private void DeleteLayer(LayerRow row)
    {
        var count = _locals.Count;
        var edit = LocalMasks.RemoveValues(_edit, row.Index, count);
        ChangeLayers(layers => layers.RemoveAt(row.Index), I18n.T("删除局部图层"), edit);
        if (ActiveLayer >= _locals.Count) ActiveLayer = _locals.Count - 1;
    }

    private void StartStroke(UvPoint point, bool erase)
    {
        var layer = _locals[ActiveLayer];
        var strokes = erase ? layer.Erase : layer.Strokes;
        strokes.Add([point]);
        _locals[ActiveLayer] = layer with { Radius = LocalBrushRadius };
        _drawing = true;
        QueueLocalMask();
    }

    private void ContinueStroke(UvPoint point)
    {
        if (!HasActiveLayer) return;
        var layer = _locals[ActiveLayer];
        var strokes = Tool == RetouchTool.RefineSubtract ? layer.Erase : layer.Strokes;
        if (strokes.Count == 0) return;
        strokes[^1].Add(point);
        QueueLocalMask();
    }

    /// <summary>Redraws the local masks, at most one at a time, always ending with the latest layers.</summary>
    private async void QueueLocalMask()
    {
        if (!_imageOpen) return;
        _maskQueued = true;
        if (_maskSending) return;
        _maskSending = true;
        var ticket = _openTicket;
        try
        {
            while (_maskQueued && ticket == _openTicket)
            {
                _maskQueued = false;
                var layers = _locals.Select(layer => layer.Copy()).ToList();
                var proxy = _proxySize;
                var masks = SelectedPhoto is { } photo && _modelMasks.TryGetValue(photo.Id, out var known) ? known : [];
                var rgba = layers.Count == 0 ? new byte[4] : await Task.Run(() => LocalMasks.Render(layers, proxy.Width, proxy.Height, masks));
                if (ticket != _openTicket) break;
                await _engine.CallAsync("setLocalMask", layers.Count == 0 ? new { width = 1, height = 1 } : new { width = proxy.Width, height = proxy.Height }, rgba);
                RequestRender();
            }
        }
        catch (EngineException exception)
        {
            Status = Messages.Friendly(exception);
        }
        finally
        {
            _maskSending = false;
        }
    }

    /// <summary>After undo or redo, the layers return to how they were at that step.</summary>
    private void RestoreLayersFromHistory()
    {
        if (_history is null || SelectedPhoto is null) return;
        _locals = _history.CurrentLocals;
        _retouch.SaveLocals(SelectedPhoto.Id, _locals);
        SyncLayerRows();
        QueueLocalMask();
        OverlayChanged?.Invoke();
    }
}
