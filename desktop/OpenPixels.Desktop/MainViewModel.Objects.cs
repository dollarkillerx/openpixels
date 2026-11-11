using CommunityToolkit.Mvvm.Input;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

/// <summary>
/// Object layers: a local mask SAM 2.1 finds from points clicked on the object (or, with
/// Option/Alt, on what to leave out) and a box dragged around it.
/// </summary>
public sealed partial class MainViewModel
{
    /// <summary>How far, in device pixels, a press must move to become a box rather than a click.</summary>
    private const double BoxDrag = 6;

    /// <summary>The SAM features of the last photo encoded, shared by every prompt on it.</summary>
    private (string Id, Task<AiModels.SegmentFeatures> Features)? _segment;

    // A press with the object tool: where it started, and the box if it has moved.
    private (UvPoint At, double X, double Y, bool Exclude)? _objectPress;
    private UvBox? _objectDrag;

    /// <summary>The open photo's local layers, as they are now.</summary>
    public IReadOnlyList<LocalLayer> LocalLayers => _locals;

    public bool IsObject => Tool == RetouchTool.Object;
    public bool ObjectAvailable => AiModels.Segment.Available;
    public bool ActiveIsObject => HasActiveLayer && _locals[ActiveLayer].Kind == LocalKind.Object;
    public bool ActiveHasPrompts => ActiveIsObject && _locals[ActiveLayer].HasPrompts;

    /// <summary>Adds an empty object layer and picks the tool that clicks on it.</summary>
    [RelayCommand]
    private async Task AddObjectLayer()
    {
        if (!CanAddLayer || !ObjectAvailable || SelectedPhoto is not { } photo) return;
        AddLayer(new LocalLayer { Kind = LocalKind.Object, A = new(0.5, 0.5), B = new(0.5, 0.5), Radius = LocalBrushRadius });
        RecordLayers(I18n.T("添加{title}", ("title", I18n.T("对象"))));
        Tool = RetouchTool.Object;
        await PrepareSegmentAsync(photo.Id);
    }

    [RelayCommand]
    private void ClearObjectPrompts()
    {
        if (!ActiveIsObject) return;
        ChangeLayers(layers => layers[ActiveLayer] = layers[ActiveLayer] with { Points = [], Box = null }, I18n.T("清除点选"));
        OverlayChanged?.Invoke();
    }

    /// <summary>Encodes the open photo ahead of the first click, so that click answers at once.</summary>
    private async Task PrepareSegmentAsync(string id)
    {
        if (_segment is { } known && known.Id == id && !known.Features.IsFaulted) return;
        var ticket = _openTicket;
        AiBusy = true;
        RetouchStatus = I18n.T("正在分析画面…");
        try
        {
            var started = Environment.TickCount64;
            await SegmentFeaturesAsync(_engine, id);
            if (ticket != _openTicket) return;
            RetouchStatus = I18n.T("已分析画面（{seconds} 秒），点击要选的对象", ("seconds", ((Environment.TickCount64 - started) / 1000.0).ToString("0.0")));
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

    private Task<AiModels.SegmentFeatures> SegmentFeaturesAsync(EngineClient engine, string id)
    {
        if (_segment is { } known && known.Id == id && !known.Features.IsFaulted) return known.Features;
        var features = EncodeAsync(engine);
        _segment = (id, features);
        return features;

        async Task<AiModels.SegmentFeatures> EncodeAsync(EngineClient from)
        {
            const int size = AiModels.Segment.Size;
            var input = await from.CallAsync("modelInput", new { width = size, height = size, mean = AiModels.Segment.Mean, std = AiModels.Segment.Std });
            var tensor = Floats(input.Data);
            return await Task.Run(() => _ai.Value.EncodeForSegment(tensor));
        }
    }

    /// <summary>
    /// Finds an object layer's mask with SAM, follows it to the photo's edges, and keeps it under
    /// the layer's prompts. Masks for prompts no longer in use are let go.
    /// </summary>
    private async Task FindObjectMaskAsync(EngineClient engine, string id, LocalLayer layer, IReadOnlyList<LocalLayer> locals, (int Width, int Height) proxy)
    {
        var features = await SegmentFeaturesAsync(engine, id);
        var points = layer.Points.Select(point => (point.X, point.Y, point.Include)).ToList();
        (double, double, double, double)? box = layer.Box is { } b ? (b.X0, b.Y0, b.X1, b.Y1) : null;
        var logits = await Task.Run(() => _ai.Value.SegmentLogits(features, points, box));
        // The coarse mask at about the model's own detail, in the photo's proportions.
        var scale = Math.Min(1, AiModels.Segment.Size / (double)Math.Max(proxy.Width, proxy.Height));
        var (width, height) = (Math.Max(1, (int)Math.Round(proxy.Width * scale)), Math.Max(1, (int)Math.Round(proxy.Height * scale)));
        var coarse = logits is null ? new byte[width * height] : await Task.Run(() => LocalMasks.FromLogits(logits, AiModels.Segment.MaskSize, width, height));
        var refined = await engine.CallAsync("refineModelMask", new { width, height }, coarse);
        if (!_modelMasks.TryGetValue(id, out var known)) _modelMasks[id] = known = [];
        known[layer.Model] = new ModelMask(refined.Data, proxy.Width, proxy.Height);
        var used = locals.Select(other => other.Model).Append(layer.Model).ToHashSet();
        foreach (var stale in known.Keys.Where(key => key.StartsWith("object:", StringComparison.Ordinal) && !used.Contains(key)).ToList()) known.Remove(stale);
    }

    private void ObjectPress(UvPoint uv, double px, double py, bool alt)
    {
        if (!ActiveIsObject)
        {
            RetouchStatus = I18n.T("先添加一个对象图层");
            return;
        }
        _objectPress = (uv, px, py, alt);
        _objectDrag = null;
        _drawing = true;
    }

    private void ObjectMove(UvPoint uv, double px, double py)
    {
        if (_objectPress is not { } press) return;
        if (_objectDrag is null && Math.Sqrt(Math.Pow(px - press.X, 2) + Math.Pow(py - press.Y, 2)) < BoxDrag * _scaling) return;
        _objectDrag = new UvBox(press.At.X, press.At.Y, uv.X, uv.Y);
        OverlayChanged?.Invoke();
    }

    /// <summary>A click adds a point; a drag sets the box. Either way SAM looks again.</summary>
    private void ObjectRelease()
    {
        if (_objectPress is not { } press || !ActiveIsObject)
        {
            _objectPress = null;
            _objectDrag = null;
            return;
        }
        var layer = _locals[ActiveLayer];
        string label;
        if (_objectDrag is { } box)
        {
            layer = layer with { Box = box };
            label = I18n.T("框选对象");
        }
        else
        {
            layer = layer with { Points = [.. layer.Points, new ObjectPoint(press.At.X, press.At.Y, !press.Exclude)] };
            label = press.Exclude ? I18n.T("排除区域") : I18n.T("点选对象");
        }
        _locals[ActiveLayer] = layer;
        _objectPress = null;
        _objectDrag = null;
        RecordLayers(label);
        RefreshLayerProperties();
        OverlayChanged?.Invoke();
    }

    /// <summary>The active object layer's points and box, and a box being dragged, on the stage.</summary>
    /// <summary>The box's corners go through the photo's geometry, so a rotated photo shows it turned.</summary>
    private (List<(Avalonia.Point At, bool Include)> Points, List<Avalonia.Point> Box) ObjectOverlay()
    {
        if (!_imageOpen || !ActiveIsObject || Tool is not (RetouchTool.Object or RetouchTool.None)) return ([], []);
        var layer = _locals[ActiveLayer];
        var points = layer.Points.Select(point => (StageOf(new UvPoint(point.X, point.Y)), point.Include)).ToList();
        List<Avalonia.Point> corners = (_objectDrag ?? layer.Box) is { } b
            ? [StageOf(new(b.X0, b.Y0)), StageOf(new(b.X1, b.Y0)), StageOf(new(b.X1, b.Y1)), StageOf(new(b.X0, b.Y1))]
            : [];
        return (points, corners);
    }
}
