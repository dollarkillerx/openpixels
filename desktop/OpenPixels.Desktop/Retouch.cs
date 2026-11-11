using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace OpenPixels.Desktop;

public readonly record struct UvPoint(double X, double Y);

/// <summary>A point clicked on an object layer, and whether it marks the object or something to leave out.</summary>
public readonly record struct ObjectPoint(double X, double Y, bool Include);

/// <summary>A box dragged around an object, in source-image UV.</summary>
public readonly record struct UvBox(double X0, double Y0, double X1, double Y1);

[JsonConverter(typeof(JsonStringEnumConverter<LocalKind>))]
public enum LocalKind { Linear, Radial, Brush, Full, Subject, Background, Sky, Object }

/// <summary>
/// A local adjustment's shape, in source-image UV. Its values live in the edit as
/// local{n}Exposure and so on. Matches LocalLayer in web/src/local.ts.
/// </summary>
public sealed record LocalLayer
{
    public LocalKind Kind { get; init; }
    /// <summary>Gradient start, or the centre of a radial.</summary>
    public UvPoint A { get; init; }
    /// <summary>Where a gradient ends, or a point on the radial's edge.</summary>
    public UvPoint B { get; init; }
    /// <summary>Brush paths; on an AI layer, what is added to the model's mask.</summary>
    public List<List<UvPoint>> Strokes { get; init; } = [];
    /// <summary>Brush paths taken away from the mask.</summary>
    public List<List<UvPoint>> Erase { get; init; } = [];
    /// <summary>Brush radius as a fraction of the long edge.</summary>
    public double Radius { get; init; } = 0.04;
    public bool Invert { get; init; }
    /// <summary>On an object layer, the points clicked for SAM.</summary>
    public List<ObjectPoint> Points { get; init; } = [];
    /// <summary>On an object layer, the box dragged around the object, if any.</summary>
    public UvBox? Box { get; init; }

    public bool IsAi => Kind is LocalKind.Subject or LocalKind.Background or LocalKind.Sky or LocalKind.Object;
    public bool HasHandles => Kind is LocalKind.Linear or LocalKind.Radial;
    public bool HasPrompts => Points.Count > 0 || Box is not null;

    /// <summary>
    /// The mask behind an AI layer; a background is the inverse of the subject. An object's mask
    /// is named by its prompts, so going back to earlier prompts finds the mask made for them.
    /// </summary>
    public string Model => Kind switch
    {
        LocalKind.Sky => "sky",
        LocalKind.Object => ObjectKey(),
        _ => "subject",
    };

    private string ObjectKey()
    {
        static string Number(double value) => value.ToString("R", System.Globalization.CultureInfo.InvariantCulture);
        var points = string.Join(';', Points.Select(point => $"{Number(point.X)},{Number(point.Y)},{(point.Include ? 1 : 0)}"));
        var box = Box is { } b ? $"{Number(b.X0)},{Number(b.Y0)},{Number(b.X1)},{Number(b.Y1)}" : "";
        return $"object:{points}|{box}";
    }

    /// <summary>A copy whose stroke and point lists can be changed without touching this one.</summary>
    public LocalLayer Copy() => this with { Strokes = Strokes.Select(stroke => stroke.ToList()).ToList(), Erase = Erase.Select(stroke => stroke.ToList()).ToList(), Points = Points.ToList() };

    public string Title => Kind switch
    {
        LocalKind.Linear => I18n.T("线性渐变"),
        LocalKind.Radial => I18n.T("径向渐变"),
        LocalKind.Brush => I18n.T("画笔"),
        LocalKind.Full => I18n.T("全图"),
        LocalKind.Subject => I18n.T("主体"),
        LocalKind.Background => I18n.T("背景"),
        LocalKind.Object => I18n.T("对象"),
        _ => I18n.T("天空"),
    };
}

/// <summary>A mask found by a model, at some size.</summary>
public sealed record ModelMask(byte[] Mask, int Width, int Height);

public static class LocalMasks
{
    public const int MaxLayers = 4;
    public static readonly (string Suffix, string Label, double Min, double Max)[] Sliders =
        [("Exposure", I18n.T("曝光"), -3, 3), ("Contrast", I18n.T("对比度"), -1, 1), ("Saturation", I18n.T("饱和度"), -1, 1), ("Temperature", I18n.T("色温"), -1, 1)];
    public static readonly string[] RangeSuffixes = ["RangeMode", "RangeLow", "RangeHigh", "RangeHue", "RangeWidth"];

    private const double RadialSoftStart = 0.55;
    private const double BrushSpacing = 0.25;

    /// <summary>Paints brush dabs (soft white discs) over a 0..1 coverage buffer, as the web's canvas does.</summary>
    private static void PaintBrush(float[] coverage, IEnumerable<List<UvPoint>> strokes, double radiusFraction, int width, int height)
    {
        var radius = Math.Max(0.5, radiusFraction * Math.Max(width, height));
        void Dab(double cx, double cy)
        {
            var (x0, x1) = (Math.Max(0, (int)Math.Floor(cx - radius)), Math.Min(width - 1, (int)Math.Ceiling(cx + radius)));
            var (y0, y1) = (Math.Max(0, (int)Math.Floor(cy - radius)), Math.Min(height - 1, (int)Math.Ceiling(cy + radius)));
            for (var y = y0; y <= y1; y++)
            {
                for (var x = x0; x <= x1; x++)
                {
                    var distance = Math.Sqrt(Math.Pow(x + 0.5 - cx, 2) + Math.Pow(y + 0.5 - cy, 2)) / radius;
                    if (distance >= 1) continue;
                    // Stops at 0, 0.6 and 1: alpha 0.6, 0.4 and 0.
                    var alpha = distance < 0.6 ? 0.6 - distance / 0.6 * 0.2 : 0.4 * (1 - (distance - 0.6) / 0.4);
                    var at = y * width + x;
                    coverage[at] += (float)(alpha * (1 - coverage[at]));
                }
            }
        }
        foreach (var stroke in strokes)
        {
            for (var index = 0; index < stroke.Count; index++)
            {
                var (x, y) = (stroke[index].X * width, stroke[index].Y * height);
                if (index == 0)
                {
                    Dab(x, y);
                    continue;
                }
                var (px, py) = (stroke[index - 1].X * width, stroke[index - 1].Y * height);
                var steps = Math.Max(1, (int)Math.Ceiling(Math.Sqrt((x - px) * (x - px) + (y - py) * (y - py)) / (radius * BrushSpacing)));
                for (var step = 1; step <= steps; step++) Dab(px + (x - px) * step / steps, py + (y - py) * step / steps);
            }
        }
    }

    private static byte[] Painted(IEnumerable<List<UvPoint>> strokes, double radius, int width, int height)
    {
        var coverage = new float[width * height];
        PaintBrush(coverage, strokes, radius, width, height);
        return coverage.Select(value => (byte)Math.Round(Math.Clamp(value, 0, 1) * 255)).ToArray();
    }

    private static byte[] Shape(LocalLayer layer, int width, int height)
    {
        var values = new byte[width * height];
        var (ax, ay, bx, by) = (layer.A.X * width, layer.A.Y * height, layer.B.X * width, layer.B.Y * height);
        switch (layer.Kind)
        {
            case LocalKind.Full:
                Array.Fill(values, (byte)255);
                return values;
            case LocalKind.Brush:
                return Painted(layer.Strokes, layer.Radius, width, height);
        }
        var (dx, dy) = (bx - ax, by - ay);
        var lengthSquared = Math.Max(dx * dx + dy * dy, 1e-9);
        var radius = Math.Max(Math.Sqrt(lengthSquared), 1);
        for (var y = 0; y < height; y++)
        {
            for (var x = 0; x < width; x++)
            {
                var (px, py) = (x + 0.5 - ax, y + 0.5 - ay);
                double value;
                if (layer.Kind == LocalKind.Linear)
                {
                    value = 1 - Math.Clamp((px * dx + py * dy) / lengthSquared, 0, 1);
                }
                else
                {
                    var distance = Math.Sqrt(px * px + py * py);
                    value = 1 - Math.Clamp((distance - radius * RadialSoftStart) / (radius * (1 - RadialSoftStart)), 0, 1);
                }
                values[y * width + x] = (byte)Math.Round(value * 255);
            }
        }
        return values;
    }

    /// <summary>A model's mask at another size, bilinearly.</summary>
    public static byte[] Resampled(ModelMask model, int outWidth, int outHeight)
    {
        var (mask, width, height) = (model.Mask, model.Width, model.Height);
        if (width == outWidth && height == outHeight) return (byte[])mask.Clone();
        var output = new byte[outWidth * outHeight];
        for (var row = 0; row < outHeight; row++)
        {
            var fy = Math.Min(height - 1, Math.Max(0, (row + 0.5) * height / outHeight - 0.5));
            var (y0, ty) = ((int)Math.Floor(fy), fy - Math.Floor(fy));
            var y1 = Math.Min(y0 + 1, height - 1);
            for (var col = 0; col < outWidth; col++)
            {
                var fx = Math.Min(width - 1, Math.Max(0, (col + 0.5) * width / outWidth - 0.5));
                var (x0, tx) = ((int)Math.Floor(fx), fx - Math.Floor(fx));
                var x1 = Math.Min(x0 + 1, width - 1);
                var top = mask[y0 * width + x0] * (1 - tx) + mask[y0 * width + x1] * tx;
                var bottom = mask[y1 * width + x0] * (1 - tx) + mask[y1 * width + x1] * tx;
                output[row * outWidth + col] = (byte)Math.Round(top * (1 - ty) + bottom * ty);
            }
        }
        return output;
    }

    /// <summary>
    /// A SAM mask's logits (square, over the whole image) as a coverage mask of the given size:
    /// bilinear in logits, as SAM upsamples, then a sigmoid, so the edge is soft but narrow.
    /// </summary>
    public static byte[] FromLogits(float[] logits, int logitSize, int width, int height)
    {
        var output = new byte[width * height];
        for (var row = 0; row < height; row++)
        {
            var fy = Math.Clamp((row + 0.5) * logitSize / height - 0.5, 0, logitSize - 1);
            var (y0, ty) = ((int)fy, fy - (int)fy);
            var y1 = Math.Min(y0 + 1, logitSize - 1);
            for (var col = 0; col < width; col++)
            {
                var fx = Math.Clamp((col + 0.5) * logitSize / width - 0.5, 0, logitSize - 1);
                var (x0, tx) = ((int)fx, fx - (int)fx);
                var x1 = Math.Min(x0 + 1, logitSize - 1);
                var top = logits[y0 * logitSize + x0] * (1 - tx) + logits[y0 * logitSize + x1] * tx;
                var bottom = logits[y1 * logitSize + x0] * (1 - tx) + logits[y1 * logitSize + x1] * tx;
                var logit = top * (1 - ty) + bottom * ty;
                output[row * width + col] = (byte)Math.Round(255 / (1 + Math.Exp(-logit)));
            }
        }
        return output;
    }

    /// <summary>A half-covered sky edge would let the old brightness through, so it is tightened.</summary>
    private static byte TightenSky(byte value)
    {
        var t = Math.Clamp((value / 255.0 - 0.3) / 0.4, 0, 1);
        return (byte)Math.Round(255 * t * t * (3 - 2 * t));
    }

    /// <summary>Rasterises up to four layers into the channels of one RGBA buffer. As renderLocalMask in the web edition.</summary>
    public static byte[] Render(IReadOnlyList<LocalLayer> layers, int width, int height, IReadOnlyDictionary<string, ModelMask> models)
    {
        var output = new byte[width * height * 4];
        for (var channel = 0; channel < Math.Min(layers.Count, MaxLayers); channel++)
        {
            var layer = layers[channel];
            byte[] values;
            if (layer.IsAi)
            {
                values = models.TryGetValue(layer.Model, out var model) ? Resampled(model, width, height) : new byte[width * height];
                if (layer.Kind == LocalKind.Background) for (var i = 0; i < values.Length; i++) values[i] = (byte)(255 - values[i]);
                if (layer.Kind == LocalKind.Sky) for (var i = 0; i < values.Length; i++) values[i] = TightenSky(values[i]);
                if (layer.Strokes.Count > 0)
                {
                    var added = Painted(layer.Strokes, layer.Radius, width, height);
                    for (var i = 0; i < values.Length; i++) values[i] = Math.Max(values[i], added[i]);
                }
            }
            else
            {
                values = Shape(layer, width, height);
            }
            if (layer.Erase.Count > 0)
            {
                var removed = Painted(layer.Erase, layer.Radius, width, height);
                for (var i = 0; i < values.Length; i++) values[i] = (byte)Math.Round(values[i] * (255 - removed[i]) / 255.0);
            }
            for (var i = 0; i < values.Length; i++) output[i * 4 + channel] = layer.Invert ? (byte)(255 - values[i]) : values[i];
        }
        return output;
    }

    /// <summary>The edit with layer index removed, and later layers' values moved down a slot.</summary>
    public static Dictionary<string, double> RemoveValues(IReadOnlyDictionary<string, double> edit, int index, int count)
    {
        var next = new Dictionary<string, double>(edit);
        for (var layer = index; layer < count; layer++)
        {
            foreach (var suffix in Sliders.Select(slider => slider.Suffix).Concat(RangeSuffixes))
            {
                if (layer + 1 < count && edit.TryGetValue($"local{layer + 1}{suffix}", out var following)) next[$"local{layer}{suffix}"] = following;
                else next.Remove($"local{layer}{suffix}");
            }
        }
        return next;
    }
}

/// <summary>One brush mark for AI erase. X and Y are fractions of the image, Radius of its long edge.</summary>
public readonly record struct EraseDab(double X, double Y, double Radius);

/// <summary>A filled-in region: where it goes, as fractions of the image, and the model's patch.</summary>
public sealed record ErasePatch(double X, double Y, double Width, double Height, int Size, byte[] Rgb, byte[] Alpha);

/// <summary>Choosing and masking the region the inpainting model sees. Ported from web/src/erase.ts.</summary>
public static class Erasing
{
    public const int ModelSize = 512;
    private const double ContextFactor = 2;
    private const double HoleGrowth = 4;
    private const double Feather = 3;

    /// <summary>The region to send to the model in image pixels: at least its input size, so small marks stay sharp.</summary>
    public static (int X, int Y, int Width, int Height) Crop(IReadOnlyList<EraseDab> dabs, int imageWidth, int imageHeight)
    {
        var longEdge = Math.Max(imageWidth, imageHeight);
        double left = double.MaxValue, top = double.MaxValue, right = double.MinValue, bottom = double.MinValue;
        foreach (var dab in dabs)
        {
            var radius = dab.Radius * longEdge;
            left = Math.Min(left, dab.X * imageWidth - radius);
            right = Math.Max(right, dab.X * imageWidth + radius);
            top = Math.Min(top, dab.Y * imageHeight - radius);
            bottom = Math.Max(bottom, dab.Y * imageHeight + radius);
        }
        var side = Math.Max(ModelSize, (int)Math.Ceiling(Math.Max(right - left, bottom - top) * ContextFactor));
        var (width, height) = (Math.Min(side, imageWidth), Math.Min(side, imageHeight));
        static int Place(double centre, int span, int limit) => Math.Min(limit - span, Math.Max(0, (int)Math.Round(centre - span / 2.0, MidpointRounding.AwayFromZero)));
        return (Place((left + right) / 2, width, imageWidth), Place((top + bottom) / 2, height, imageHeight), width, height);
    }

    /// <summary>The model's binary hole and the patch's alpha over the crop.</summary>
    public static (float[] Mask, byte[] Alpha) Rasterize(IReadOnlyList<EraseDab> dabs, (int X, int Y, int Width, int Height) crop, int imageWidth, int imageHeight)
    {
        const int size = ModelSize;
        var mask = new float[size * size];
        var alpha = new byte[size * size];
        var longEdge = Math.Max(imageWidth, imageHeight);
        var (scaleX, scaleY) = (size / (double)crop.Width, size / (double)crop.Height);
        // Margins are set in model pixels and measured in image pixels.
        var growth = HoleGrowth / Math.Min(scaleX, scaleY);
        var feather = Feather / Math.Min(scaleX, scaleY);
        foreach (var dab in dabs)
        {
            var reach = dab.Radius * longEdge + growth;
            var (centreX, centreY) = (dab.X * imageWidth - crop.X, dab.Y * imageHeight - crop.Y);
            var x0 = Math.Max(0, (int)Math.Floor((centreX - reach) * scaleX));
            var x1 = Math.Min(size, (int)Math.Ceiling((centreX + reach) * scaleX));
            var y0 = Math.Max(0, (int)Math.Floor((centreY - reach) * scaleY));
            var y1 = Math.Min(size, (int)Math.Ceiling((centreY + reach) * scaleY));
            for (var y = y0; y < y1; y++)
            {
                for (var x = x0; x < x1; x++)
                {
                    var distance = Math.Sqrt(Math.Pow((x + 0.5) / scaleX - centreX, 2) + Math.Pow((y + 0.5) / scaleY - centreY, 2));
                    if (distance > reach) continue;
                    var at = y * size + x;
                    mask[at] = 1;
                    var t = Math.Min(1, (reach - distance) / feather);
                    alpha[at] = Math.Max(alpha[at], (byte)Math.Round(255 * t * t * (3 - 2 * t)));
                }
            }
        }
        return (mask, alpha);
    }
}

/// <summary>Heal spots, erase patches and local layers, kept per photo beside the edits.</summary>
public sealed class RetouchStore
{
    private static readonly JsonSerializerOptions Options = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };

    private readonly JsonFile<Dictionary<string, JsonArray>> _healFile = new("heals.json");
    private readonly JsonFile<Dictionary<string, List<LocalLayer>>> _localFile = new("locals.json");
    private readonly JsonFile<Dictionary<string, List<JsonArray>>> _warpFile = new("warps.json");
    private readonly Dictionary<string, List<JsonArray>> _warps;
    private readonly Dictionary<string, JsonArray> _heals;
    private readonly Dictionary<string, List<LocalLayer>> _locals;
    private readonly string _eraseFolder = Path.Combine(DataFolder.Path, "erases");

    public RetouchStore()
    {
        _heals = _healFile.Load();
        _locals = _localFile.Load();
        _warps = _warpFile.Load();
    }

    /// <summary>Spots as the engine's HealSpot JSON objects, plus an optional "group" for one stamp drag.</summary>
    public JsonArray Heals(string id) => _heals.TryGetValue(id, out var spots) ? (JsonArray)spots.DeepClone() : [];

    public void SaveHeals(string id, JsonArray spots)
    {
        if (spots.Count > 0) _heals[id] = (JsonArray)spots.DeepClone();
        else _heals.Remove(id);
        _healFile.Save(_heals);
    }

    /// <summary>Liquify strokes, grouped by the drag that made them so undo removes a whole drag.</summary>
    public List<JsonArray> Warps(string id) => _warps.TryGetValue(id, out var groups) ? groups.Select(group => (JsonArray)group.DeepClone()).ToList() : [];

    public void SaveWarps(string id, IReadOnlyList<JsonArray> groups)
    {
        if (groups.Count > 0) _warps[id] = groups.Select(group => (JsonArray)group.DeepClone()).ToList();
        else _warps.Remove(id);
        _warpFile.Save(_warps);
    }

    public List<LocalLayer> Locals(string id) => _locals.TryGetValue(id, out var layers) ? layers.Select(layer => layer.Copy()).ToList() : [];

    public void SaveLocals(string id, IReadOnlyList<LocalLayer> layers)
    {
        if (layers.Count > 0) _locals[id] = layers.Select(layer => layer.Copy()).ToList();
        else _locals.Remove(id);
        _localFile.Save(_locals);
    }

    private string EraseFile(string id) => Path.Combine(_eraseFolder, Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(id)))[..32] + ".json");

    public List<ErasePatch> Erases(string id)
    {
        try
        {
            var file = EraseFile(id);
            return File.Exists(file) ? JsonSerializer.Deserialize<List<ErasePatch>>(File.ReadAllText(file), Options) ?? [] : [];
        }
        catch (JsonException)
        {
            return [];
        }
    }

    public void SaveErases(string id, IReadOnlyList<ErasePatch> patches)
    {
        var file = EraseFile(id);
        if (patches.Count == 0)
        {
            File.Delete(file);
            return;
        }
        Directory.CreateDirectory(_eraseFolder);
        File.WriteAllText(file, JsonSerializer.Serialize(patches, Options));
    }
}
