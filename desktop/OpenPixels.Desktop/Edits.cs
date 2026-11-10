using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace OpenPixels.Desktop;

/// <summary>One slider: an edit key shared with the web edition, and its range.</summary>
public sealed record SliderSpec(string Key, string Label, double Min, double Max, double Resting = 0);

public sealed record SliderGroupSpec(string Title, IReadOnlyList<SliderSpec> Sliders);

public sealed record HslBand(string Name, string Colour);

/// <summary>The edit model. Keys and ranges match the web edition, so edits mean the same in both.</summary>
public static partial class Edits
{
    public static readonly SliderGroupSpec WhiteBalance = new("白平衡", [new("temperature", "色温", -2, 2), new("tint", "色调", -2, 2)]);

    public static readonly SliderGroupSpec Tone = new("影调",
    [
        new("exposure", "曝光", -4, 4), new("contrast", "对比度", -1, 1), new("highlights", "高光", -1, 1),
        new("shadows", "阴影", -1, 1), new("whites", "白色", -1, 1), new("blacks", "黑色", -1, 1),
    ]);

    public static readonly SliderGroupSpec Parametric = new("参数曲线",
    [
        new("curveHighlights", "曲线高光", -1, 1), new("curveLights", "曲线亮调", -1, 1),
        new("curveDarks", "曲线暗调", -1, 1), new("curveShadows", "曲线阴影", -1, 1),
    ]);

    public static readonly SliderGroupSpec Effects = new("色彩与效果",
    [
        new("vibrance", "自然饱和度", -1, 1), new("saturation", "饱和度", -1, 1), new("dehaze", "去雾", -1, 1),
        new("clarity", "清晰度", -1, 1), new("texture", "纹理", -1, 1),
        new("sharpen", "锐化", 0, 1), new("sharpenRadius", "锐化半径", 1, 3, 1), new("sharpenDetail", "锐化细节", 0, 1),
        new("sharpenMasking", "锐化蒙版", 0, 1), new("denoiseLuma", "明度降噪", 0, 1), new("denoiseChroma", "色彩降噪", 0, 1),
        new("vignette", "暗角", -1, 1), new("postCropVignette", "裁剪后暗角", -1, 1),
        new("grain", "颗粒", 0, 1), new("grainSize", "颗粒大小", 0, 1),
    ]);

    public static readonly SliderGroupSpec Calibration = new("校准",
    [
        new("calibrationRedHue", "红原色色相", -1, 1), new("calibrationRedSat", "红原色饱和度", -1, 1),
        new("calibrationGreenHue", "绿原色色相", -1, 1), new("calibrationGreenSat", "绿原色饱和度", -1, 1),
        new("calibrationBlueHue", "蓝原色色相", -1, 1), new("calibrationBlueSat", "蓝原色饱和度", -1, 1),
    ]);

    public static readonly SliderGroupSpec Lens = new("镜头校正",
    [
        new("lensDistortion", "畸变", -1, 1), new("lensVignette", "暗角补偿", -1, 1),
        new("lensCaRed", "红/青边", -1, 1), new("lensCaBlue", "蓝/黄边", -1, 1),
    ]);

    public static readonly SliderGroupSpec Perspective = new("透视矫正", [new("perspectiveV", "垂直透视", -1, 1), new("perspectiveH", "水平透视", -1, 1)]);

    public static readonly SliderGroupSpec Point = new("颜色混合点",
    [
        new("pointWidth", "混合点范围", 10, 90, 30), new("pointHueShift", "混合点色相", -1, 1),
        new("pointSatShift", "混合点饱和度", -1, 1), new("pointLumShift", "混合点明度", -1, 1),
    ]);

    public static readonly SliderSpec LookAmount = new("lutAmount", "滤镜强度", 0, 1);
    public static readonly SliderSpec Rotate = new("rotate", "旋转", -45, 45);

    public static readonly HslBand[] HslBands =
    [
        new("红", "#e0483e"), new("橙", "#e88a2e"), new("黄", "#e5cb3a"), new("绿", "#55b04a"),
        new("青", "#3fbcc0"), new("蓝", "#3f73d8"), new("紫", "#7e52c8"), new("品红", "#cc4aa8"),
    ];

    public static readonly (string Prefix, string Label)[] HslChannels = [("hslHue", "色相"), ("hslSat", "饱和度"), ("hslLum", "明度")];

    public static readonly (string Key, string Label)[] GradeRanges = [("Shadow", "阴影"), ("Mid", "中间调"), ("High", "高光")];

    /// <summary>All HSL sliders, band by band.</summary>
    public static IEnumerable<SliderSpec> HslSliders() =>
        HslBands.SelectMany((band, index) => HslChannels.Select(channel => new SliderSpec($"{channel.Prefix}{index}", $"{band.Name}{channel.Label}", -1, 1)));

    private static double Get(IReadOnlyDictionary<string, double> edit, string key, double fallback = 0) =>
        edit.TryGetValue(key, out var value) && double.IsFinite(value) ? value : fallback;

    /// <summary>
    /// Serialises an edit for the engine, the same way the web edition's engineJson does.
    /// Non-finite values become 0, since the engine rejects null.
    /// </summary>
    public static string ToEngineJson(IReadOnlyDictionary<string, double> edit, bool cropEditing = false)
    {
        JsonArray Numbers(IEnumerable<double> values) => new(values.Select(value => (JsonNode)JsonValue.Create(double.IsFinite(value) ? value : 0)!).ToArray());
        JsonArray Bands(string prefix) => new(Numbers(Enumerable.Range(0, 4).Select(i => Get(edit, $"{prefix}{i}"))), Numbers(Enumerable.Range(4, 4).Select(i => Get(edit, $"{prefix}{i}"))));

        var json = new JsonObject();
        foreach (var (key, value) in edit) json[key] = double.IsFinite(value) ? value : 0;
        if (cropEditing) json["cropEditing"] = 1;
        json["quarter"] = (int)Get(edit, "quarter");
        json["hslHue"] = Bands("hslHue");
        json["hslSaturation"] = Bands("hslSat");
        json["hslLuminance"] = Bands("hslLum");
        json["curve"] = new JsonArray(Curves.Channels.Select(channel =>
            (JsonNode)new JsonArray(Curves.PointsOf(edit, channel).Select(point => (JsonNode)Numbers([point.X, point.Y])).ToArray())).ToArray());
        json["parametric"] = Numbers(Curves.ParametricAmounts(edit));
        json["lensManual"] = Numbers([Get(edit, "lensDistortion"), Get(edit, "lensVignette"), Get(edit, "lensCaRed"), Get(edit, "lensCaBlue")]);
        json["perspective"] = Numbers([Get(edit, "perspectiveV"), Get(edit, "perspectiveH")]);
        json["grade"] = new JsonArray(GradeRanges.Select(range => (JsonNode)Numbers([Get(edit, $"grade{range.Key}Hue"), Get(edit, $"grade{range.Key}Amount"), 0, 0])).ToArray());
        json["localRange"] = new JsonArray(Enumerable.Range(0, 4).Select(_ => (JsonNode)Numbers([0, 1, 0, 0])).ToArray());
        json["local"] = new JsonArray(Enumerable.Range(0, 4).Select(_ => (JsonNode)Numbers([0, 0, 0, 0])).ToArray());
        return json.ToJsonString();
    }

    /// <summary>Framing and masks belong to one photo, so presets and pasting leave them out.</summary>
    public static bool IsFraming(string key) => key.StartsWith("crop", StringComparison.Ordinal) || key is "quarter" or "rotate";

    // History names, as in the web edition's describeChange.

    private static readonly Dictionary<string, string> SliderLabels =
        new[] { WhiteBalance, Tone, Parametric, Effects, Calibration, Lens, Perspective, Point }
            .SelectMany(group => group.Sliders).Concat(HslSliders()).Concat([LookAmount, Rotate])
            .ToDictionary(slider => slider.Key, slider => slider.Label);

    private static readonly Dictionary<string, string> CurveNames = new() { ["M"] = "RGB 曲线", ["R"] = "红色曲线", ["G"] = "绿色曲线", ["B"] = "蓝色曲线" };

    private const string CropGroup = "裁剪与旋转";
    private const string PointGroup = "颜色混合点";

    private sealed record SettingName(string Label, string? Group = null, bool Numeric = false);

    [GeneratedRegex(@"^curve([MRGB])(?:\d|N|[XY]\d+)$")]
    private static partial Regex CurveKey();

    [GeneratedRegex(@"^grade(Shadow|Mid|High)(Hue|Amount)$")]
    private static partial Regex GradeKey();

    private static SettingName? NameOf(string key)
    {
        SliderLabels.TryGetValue(key, out var slider);
        if (key is "temperature" or "tint") return new(slider!, "白平衡", true);
        if (key == "pointHue") return new("混合点取色", PointGroup);
        if (key.StartsWith("point", StringComparison.Ordinal) && slider is not null) return new(slider, PointGroup, true);
        if (key == "rotate") return new("旋转", CropGroup, true);
        if (key == "lutAmount") return new("滤镜强度", "滤镜", true);
        if (slider is not null) return new(slider, null, true);
        if (key == "quarter" || (key.Length == 5 && key.StartsWith("crop", StringComparison.Ordinal))) return new(CropGroup, CropGroup);
        if (key == "lutId") return new("滤镜", "滤镜");
        if (key == "cameraProfile") return new("色彩配置");
        if (key == "lensAuto") return new("自动镜头校正");
        if (key == "perspectiveKeep") return new("保留全图");
        if (CurveKey().Match(key) is { Success: true } curve)
        {
            var name = CurveNames[curve.Groups[1].Value];
            return new(name, name);
        }
        if (GradeKey().Match(key) is { Success: true } grade)
        {
            var label = GradeRanges.First(range => range.Key == grade.Groups[1].Value).Label + "色轮";
            return new(label, label);
        }
        return null;
    }

    /// <summary>Names the change between two states of an edit, for the history list.</summary>
    public static string DescribeChange(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after)
    {
        var changed = before.Keys.Union(after.Keys).Where(key => Get(before, key) != Get(after, key)).Order(StringComparer.Ordinal).ToList();
        if (changed.Count == 0) return "无变化";
        var names = changed.Select(NameOf).ToList();
        if (changed.Count > 1 && names[0]?.Group is { } group && names.All(name => name?.Group == group)) return group;
        if (changed.Count > 3) return $"多项调整（{changed.Count}）";
        var position = names.FindIndex(name => name is not null);
        if (position < 0) return "调整";
        var (label, _, numeric) = names[position]!;
        if (!numeric) return label;
        var delta = Get(after, changed[position]) - Get(before, changed[position]);
        return $"{label} {(delta > 0 ? "+" : "")}{delta:0.00}";
    }

    /// <summary>The keys whose value differs, so consecutive changes to the same thing can merge.</summary>
    public static string ChangedKeys(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after) =>
        string.Join(',', before.Keys.Union(after.Keys).Where(key => Get(before, key) != Get(after, key)).Order(StringComparer.Ordinal));
}

/// <summary>Where the desktop edition keeps its data: the user's application data, or OPENPIXELS_DATA_DIR.</summary>
public static class DataFolder
{
    /// <summary>Read on each use, so a changed OPENPIXELS_DATA_DIR takes effect for new stores.</summary>
    public static string Path => Create();

    private static string Create()
    {
        // OPENPIXELS_DATA_DIR keeps tests and portable setups out of the user's own data.
        var folder = Environment.GetEnvironmentVariable("OPENPIXELS_DATA_DIR") is { Length: > 0 } custom
            ? custom
            : System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "OpenPixels");
        Directory.CreateDirectory(folder);
        return folder;
    }
}

/// <summary>A value saved as one JSON file, replaced atomically on each save.</summary>
public sealed class JsonFile<T>(string name) where T : new()
{
    private readonly string _file = Path.Combine(DataFolder.Path, name);

    public T Load()
    {
        try
        {
            return File.Exists(_file) ? JsonSerializer.Deserialize<T>(File.ReadAllText(_file)) ?? new T() : new T();
        }
        catch (JsonException)
        {
            // A damaged file must not stop the app; it is replaced on the next save.
            return new T();
        }
    }

    public void Save(T value)
    {
        var temporary = _file + ".tmp";
        File.WriteAllText(temporary, JsonSerializer.Serialize(value));
        File.Move(temporary, _file, overwrite: true);
    }
}

public sealed record Snapshot(string Name, Dictionary<string, double> Edit);

/// <summary>Edits saved per photo, keyed like the web edition by name, size and modification time.</summary>
public sealed class EditStore
{
    private readonly JsonFile<Dictionary<string, Dictionary<string, double>>> _file = new("edits.json");
    private readonly JsonFile<Dictionary<string, List<Snapshot>>> _snapshotFile = new("snapshots.json");
    private readonly Dictionary<string, Dictionary<string, double>> _edits;
    private readonly Dictionary<string, List<Snapshot>> _snapshots;

    public EditStore()
    {
        _edits = _file.Load();
        _snapshots = _snapshotFile.Load();
    }

    public static string IdOf(string path)
    {
        var info = new FileInfo(path);
        return $"{info.FullName}|{info.Length}|{info.LastWriteTimeUtc.Ticks}";
    }

    public Dictionary<string, double> Load(string id) => _edits.TryGetValue(id, out var edit) ? new(edit) : [];

    public static Dictionary<string, double> Kept(IReadOnlyDictionary<string, double> edit) =>
        edit.Where(pair => pair.Value != 0 && double.IsFinite(pair.Value)).ToDictionary();

    public void Save(string id, IReadOnlyDictionary<string, double> edit)
    {
        var kept = Kept(edit);
        if (kept.Count > 0) _edits[id] = kept;
        else _edits.Remove(id);
        _file.Save(_edits);
    }

    public List<Snapshot> Snapshots(string id) => _snapshots.TryGetValue(id, out var list) ? [.. list] : [];

    public void SaveSnapshots(string id, IReadOnlyList<Snapshot> snapshots)
    {
        if (snapshots.Count > 0) _snapshots[id] = [.. snapshots];
        else _snapshots.Remove(id);
        _snapshotFile.Save(_snapshots);
    }
}
