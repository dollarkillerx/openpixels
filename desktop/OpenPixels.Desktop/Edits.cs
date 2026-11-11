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
    public static readonly SliderGroupSpec WhiteBalance = new(I18n.T("白平衡"), [new("temperature", I18n.T("色温"), -2, 2), new("tint", I18n.T("色调"), -2, 2)]);

    public static readonly SliderGroupSpec Tone = new(I18n.T("影调"),
    [
        new("exposure", I18n.T("曝光"), -4, 4), new("contrast", I18n.T("对比度"), -1, 1), new("highlights", I18n.T("高光"), -1, 1),
        new("shadows", I18n.T("阴影"), -1, 1), new("whites", I18n.T("白色"), -1, 1), new("blacks", I18n.T("黑色"), -1, 1),
    ]);

    public static readonly SliderGroupSpec Parametric = new(I18n.T("参数曲线"),
    [
        new("curveHighlights", I18n.T("曲线高光"), -1, 1), new("curveLights", I18n.T("曲线亮调"), -1, 1),
        new("curveDarks", I18n.T("曲线暗调"), -1, 1), new("curveShadows", I18n.T("曲线阴影"), -1, 1),
    ]);

    public static readonly SliderGroupSpec Effects = new(I18n.T("色彩与效果"),
    [
        new("vibrance", I18n.T("自然饱和度"), -1, 1), new("saturation", I18n.T("饱和度"), -1, 1), new("dehaze", I18n.T("去雾"), -1, 1),
        new("clarity", I18n.T("清晰度"), -1, 1), new("texture", I18n.T("纹理"), -1, 1),
        new("sharpen", I18n.T("锐化"), 0, 1), new("sharpenRadius", I18n.T("锐化半径"), 1, 3, 1), new("sharpenDetail", I18n.T("锐化细节"), 0, 1),
        new("sharpenMasking", I18n.T("锐化蒙版"), 0, 1), new("denoiseLuma", I18n.T("明度降噪"), 0, 1), new("denoiseChroma", I18n.T("色彩降噪"), 0, 1),
        new("vignette", I18n.T("暗角"), -1, 1), new("postCropVignette", I18n.T("裁剪后暗角"), -1, 1),
        new("grain", I18n.T("颗粒"), 0, 1), new("grainSize", I18n.T("颗粒大小"), 0, 1),
    ]);

    public static readonly SliderGroupSpec Calibration = new(I18n.T("校准"),
    [
        new("calibrationRedHue", I18n.T("红原色色相"), -1, 1), new("calibrationRedSat", I18n.T("红原色饱和度"), -1, 1),
        new("calibrationGreenHue", I18n.T("绿原色色相"), -1, 1), new("calibrationGreenSat", I18n.T("绿原色饱和度"), -1, 1),
        new("calibrationBlueHue", I18n.T("蓝原色色相"), -1, 1), new("calibrationBlueSat", I18n.T("蓝原色饱和度"), -1, 1),
    ]);

    public static readonly SliderGroupSpec Lens = new(I18n.T("镜头校正"),
    [
        new("lensDistortion", I18n.T("畸变"), -1, 1), new("lensVignette", I18n.T("暗角补偿"), -1, 1),
        new("lensCaRed", I18n.T("红/青边"), -1, 1), new("lensCaBlue", I18n.T("蓝/黄边"), -1, 1),
    ]);

    public static readonly SliderGroupSpec Perspective = new(I18n.T("透视矫正"), [new("perspectiveV", I18n.T("垂直透视"), -1, 1), new("perspectiveH", I18n.T("水平透视"), -1, 1)]);

    public static readonly SliderGroupSpec Point = new(I18n.T("颜色混合点"),
    [
        new("pointWidth", I18n.T("混合点范围"), 10, 90, 30), new("pointHueShift", I18n.T("混合点色相"), -1, 1),
        new("pointSatShift", I18n.T("混合点饱和度"), -1, 1), new("pointLumShift", I18n.T("混合点明度"), -1, 1),
    ]);

    public static readonly SliderGroupSpec Film = new(I18n.T("胶片模拟"),
    [
        new("filmExposure", I18n.T("胶片曝光"), -2, 2), new("filmLightRed", I18n.T("印片红光"), -1, 1),
        new("filmLightGreen", I18n.T("印片绿光"), -1, 1), new("filmLightBlue", I18n.T("印片蓝光"), -1, 1),
        new("filmHalation", I18n.T("光晕"), 0, 1), new("filmGrain", I18n.T("胶片颗粒"), 0, 2),
    ]);

    public static readonly SliderGroupSpec Skin = new(I18n.T("皮肤"), [new("skinSmooth", I18n.T("磨皮"), 0, 1), new("skinBlemish", I18n.T("祛斑"), 0, 1)]);

    public static readonly SliderGroupSpec BackgroundReplace = new(I18n.T("换背景"),
    [
        new("backgroundAmount", I18n.T("融合强度"), 0, 1, 1), new("backgroundFillExposure", I18n.T("亮度"), -3, 3),
        new("backgroundShift", I18n.T("上下位置"), -0.5, 0.5), new("backgroundExposure", I18n.T("背景曝光"), -4, 4),
    ]);

    public static readonly SliderGroupSpec SkyReplace = new(I18n.T("换天"),
    [
        new("skyAmount", I18n.T("融合强度"), 0, 1, 1), new("skyFillExposure", I18n.T("亮度"), -3, 3),
        new("skyShift", I18n.T("上下位置"), -0.5, 0.5), new("skyRelight", I18n.T("前景融合"), 0, 1),
    ]);

    /// <summary>Settings whose untouched value is not 0, so a 0 must be stored. As in the web edition.</summary>
    public static readonly HashSet<string> ZeroIsAChoice = ["backgroundAmount", "skyAmount"];

    public static readonly SliderSpec LookAmount = new("lutAmount", I18n.T("滤镜强度"), 0, 1);
    public static readonly SliderSpec Rotate = new("rotate", I18n.T("旋转"), -45, 45);

    public static readonly HslBand[] HslBands =
    [
        new(I18n.T("红"), "#e0483e"), new(I18n.T("橙"), "#e88a2e"), new(I18n.T("黄"), "#e5cb3a"), new(I18n.T("绿"), "#55b04a"),
        new(I18n.T("青"), "#3fbcc0"), new(I18n.T("蓝"), "#3f73d8"), new(I18n.T("紫"), "#7e52c8"), new(I18n.T("品红"), "#cc4aa8"),
    ];

    public static readonly (string Prefix, string Label)[] HslChannels = [("hslHue", I18n.T("色相")), ("hslSat", I18n.T("饱和度")), ("hslLum", I18n.T("明度"))];

    public static readonly (string Key, string Label)[] GradeRanges = [("Shadow", I18n.T("阴影")), ("Mid", I18n.T("中间调")), ("High", I18n.T("高光"))];

    /// <summary>All HSL sliders, band by band.</summary>
    public static IEnumerable<SliderSpec> HslSliders() =>
        HslBands.SelectMany((band, index) => HslChannels.Select(channel => new SliderSpec($"{channel.Prefix}{index}", $"{band.Name}{channel.Label}", -1, 1)));

    private static double Get(IReadOnlyDictionary<string, double> edit, string key, double fallback = 0) =>
        edit.TryGetValue(key, out var value) && double.IsFinite(value) ? value : fallback;

    /// <summary>
    /// Serialises an edit for the engine, the same way the web edition's engineJson does.
    /// Non-finite values become 0, since the engine rejects null.
    /// </summary>
    public static string ToEngineJson(IReadOnlyDictionary<string, double> edit, bool cropEditing = false, int localOverlay = 0)
    {
        JsonArray Numbers(IEnumerable<double> values) => new(values.Select(value => (JsonNode)JsonValue.Create(double.IsFinite(value) ? value : 0)!).ToArray());
        JsonArray Bands(string prefix) => new(Numbers(Enumerable.Range(0, 4).Select(i => Get(edit, $"{prefix}{i}"))), Numbers(Enumerable.Range(4, 4).Select(i => Get(edit, $"{prefix}{i}"))));

        var json = new JsonObject();
        foreach (var (key, value) in edit) json[key] = double.IsFinite(value) ? value : 0;
        if (cropEditing) json["cropEditing"] = 1;
        // 1-based layer whose mask is tinted on screen; never part of a saved edit.
        if (localOverlay > 0) json["localOverlay"] = localOverlay;
        json["quarter"] = (int)Get(edit, "quarter");
        json["hslHue"] = Bands("hslHue");
        json["hslSaturation"] = Bands("hslSat");
        json["hslLuminance"] = Bands("hslLum");
        json["curve"] = new JsonArray(Curves.Channels.Select(channel =>
            (JsonNode)new JsonArray(Curves.PointsOf(edit, channel).Select(point => (JsonNode)Numbers([point.X, point.Y])).ToArray())).ToArray());
        json["parametric"] = Numbers(Curves.ParametricAmounts(edit));
        json["replaceBackground"] = Get(edit, "backgroundAsset") > 0 ? Get(edit, "backgroundAmount", 1) : 0;
        json["replaceBackgroundExposure"] = Get(edit, "backgroundFillExposure");
        json["replaceBackgroundShift"] = Get(edit, "backgroundShift");
        json["replaceSky"] = Get(edit, "skyAsset") > 0 ? Get(edit, "skyAmount", 1) : 0;
        json["replaceSkyExposure"] = Get(edit, "skyFillExposure");
        json["replaceSkyShift"] = Get(edit, "skyShift");
        json["replaceSkyRelight"] = Get(edit, "skyRelight");
        json["film"] = Numbers([Get(edit, "filmExposure"), Get(edit, "filmHalation"), Get(edit, "filmGrain"), FilmSim.FormatsMm[FilmSim.FormatIndex(edit)]]);
        json["filmLights"] = Numbers([Get(edit, "filmLightRed"), Get(edit, "filmLightGreen"), Get(edit, "filmLightBlue"), 0]);
        json["lensManual"] = Numbers([Get(edit, "lensDistortion"), Get(edit, "lensVignette"), Get(edit, "lensCaRed"), Get(edit, "lensCaBlue")]);
        json["perspective"] = Numbers([Get(edit, "perspectiveV"), Get(edit, "perspectiveH")]);
        json["grade"] = new JsonArray(GradeRanges.Select(range => (JsonNode)Numbers([Get(edit, $"grade{range.Key}Hue"), Get(edit, $"grade{range.Key}Amount"), 0, 0])).ToArray());
        json["localRange"] = new JsonArray(Enumerable.Range(0, 4).Select(layer =>
        {
            double Value(string name, double fallback) => Get(edit, $"local{layer}Range{name}", fallback);
            return (JsonNode)((int)Value("Mode", 0) switch
            {
                1 => Numbers([Value("Low", 0), Value("High", 1), 0, 0]),
                2 => Numbers([0, 1, Value("Hue", 0), Value("Width", 30)]),
                _ => Numbers([0, 1, 0, 0]),
            });
        }).ToArray());
        json["local"] = new JsonArray(Enumerable.Range(0, 4).Select(layer =>
            (JsonNode)Numbers(new[] { "Exposure", "Contrast", "Saturation", "Temperature" }.Select(name => Get(edit, $"local{layer}{name}")))).ToArray());
        return json.ToJsonString();
    }

    /// <summary>
    /// Framing and local layers belong to one photo, so presets, pasting and syncing leave
    /// them out. Matches NOT_SYNCED in the web edition.
    /// </summary>
    public static bool BelongsToOnePhoto(string key) =>
        key.StartsWith("crop", StringComparison.Ordinal) || key is "quarter" or "rotate" || (key.StartsWith("local", StringComparison.Ordinal) && key.Length > 5 && char.IsDigit(key[5]));

    /// <summary>A photo's edit with source's settings, keeping what belongs only to that photo.</summary>
    public static Dictionary<string, double> Merge(IReadOnlyDictionary<string, double> source, IReadOnlyDictionary<string, double> target)
    {
        var merged = target.Where(pair => BelongsToOnePhoto(pair.Key)).ToDictionary();
        foreach (var (key, value) in source)
        {
            if (!BelongsToOnePhoto(key)) merged[key] = value;
        }
        return merged;
    }

    // History names, as in the web edition's describeChange.

    private static readonly Dictionary<string, string> SliderLabels =
        new[] { WhiteBalance, Tone, Parametric, Effects, Calibration, Lens, Perspective, Point, Film, Skin, BackgroundReplace, SkyReplace }
            .SelectMany(group => group.Sliders).Concat(HslSliders()).Concat([LookAmount, Rotate])
            .ToDictionary(slider => slider.Key, slider => slider.Label);

    private static readonly Dictionary<string, string> CurveNames = new() { ["M"] = I18n.T("RGB 曲线"), ["R"] = I18n.T("红色曲线"), ["G"] = I18n.T("绿色曲线"), ["B"] = I18n.T("蓝色曲线") };

    private static readonly string CropGroup = I18n.T("裁剪与旋转");
    private static readonly string PointGroup = I18n.T("颜色混合点");

    private sealed record SettingName(string Label, string? Group = null, bool Numeric = false);

    [GeneratedRegex(@"^curve([MRGB])(?:\d|N|[XY]\d+)$")]
    private static partial Regex CurveKey();

    [GeneratedRegex(@"^grade(Shadow|Mid|High)(Hue|Amount)$")]
    private static partial Regex GradeKey();

    private static SettingName? NameOf(string key)
    {
        SliderLabels.TryGetValue(key, out var slider);
        if (key is "temperature" or "tint") return new(slider!, I18n.T("白平衡"), true);
        if (key == "pointHue") return new(I18n.T("混合点取色"), PointGroup);
        if (key.StartsWith("point", StringComparison.Ordinal) && slider is not null) return new(slider, PointGroup, true);
        if (key == "rotate") return new(I18n.T("旋转"), CropGroup, true);
        if (key == "lutAmount") return new(I18n.T("滤镜强度"), I18n.T("滤镜"), true);
        if (slider is not null) return new(slider, null, true);
        if (key == "quarter" || (key.Length == 5 && key.StartsWith("crop", StringComparison.Ordinal))) return new(CropGroup, CropGroup);
        if (key == "lutId") return new(I18n.T("滤镜"), I18n.T("滤镜"));
        if (key == "filmPrint") return new(I18n.T("相纸"), I18n.T("胶片模拟"));
        if (key == "filmFormat") return new(I18n.T("画幅"), I18n.T("胶片模拟"));
        if (key == "cameraProfile") return new(I18n.T("色彩配置"));
        if (key == "backgroundAsset") return new(I18n.T("换背景"));
        if (key == "skyAsset") return new(I18n.T("换天"));
        if (key == "lensAuto") return new(I18n.T("自动镜头校正"));
        if (key == "perspectiveKeep") return new(I18n.T("保留全图"));
        if (CurveKey().Match(key) is { Success: true } curve)
        {
            var name = CurveNames[curve.Groups[1].Value];
            return new(name, name);
        }
        if (key.StartsWith("local", StringComparison.Ordinal) && key.Length > 6 && char.IsDigit(key[5]))
        {
            var suffix = key[6..];
            if (LocalMasks.Sliders.FirstOrDefault(slider => slider.Suffix == suffix) is { Label: { } local }) return new(I18n.T("局部{local}", ("local", local)), null, true);
            if (LocalMasks.RangeSuffixes.Contains(suffix)) return new(I18n.T("局部范围"), I18n.T("局部范围"));
        }
        if (GradeKey().Match(key) is { Success: true } grade)
        {
            var label = I18n.T("{range}色轮", ("range", GradeRanges.First(range => range.Key == grade.Groups[1].Value).Label));
            return new(label, label);
        }
        return null;
    }

    /// <summary>Names the change between two states of an edit, for the history list.</summary>
    public static string DescribeChange(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after)
    {
        var changed = before.Keys.Union(after.Keys).Where(key => Get(before, key) != Get(after, key)).Order(StringComparer.Ordinal).ToList();
        if (changed.Count == 0) return I18n.T("无变化");
        var names = changed.Select(NameOf).ToList();
        if (changed.Count > 1 && names[0]?.Group is { } group && names.All(name => name?.Group == group)) return group;
        if (changed.Count > 3) return I18n.T("多项调整（{count}）", ("count", changed.Count));
        var position = names.FindIndex(name => name is not null);
        if (position < 0) return I18n.T("调整");
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
        edit.Where(pair => (pair.Value != 0 || Edits.ZeroIsAChoice.Contains(pair.Key)) && double.IsFinite(pair.Value)).ToDictionary();

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
