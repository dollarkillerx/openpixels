using System.Globalization;

namespace OpenPixels.Desktop;

/// <summary>A 3D colour lookup table: size^3 RGB triples in 0..1, with red varying fastest.</summary>
public sealed record Lut(int Size, float[] Data);

public sealed record LookInfo(int Id, string Group, string Name)
{
    public override string ToString() => Id == 0 ? Name : $"{Group} · {Name}";
}

/// <summary>
/// The built-in looks, generated from the same recipes as web/src/lut.ts so a look id means
/// the same thing in both editions, and imported .cube files.
/// </summary>
public static class Looks
{
    private const int BuiltInSize = 25;
    private const string BasicGroup = "基础";
    /// <summary>Imported looks are numbered from here, as in the web edition.</summary>
    public const int FirstImportedId = 100;

    private delegate (double R, double G, double B) Grade(double r, double g, double b);

    private static double Luma(double r, double g, double b) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
    private static double SCurve(double v, double amount) => v + amount * (v - 0.5) * (1 - Math.Abs(2 * v - 1)) * 2;
    private static double Clamp01(double v) => Math.Min(Math.Max(v, 0), 1);

    private static Lut Generate(Grade grade)
    {
        const int size = BuiltInSize;
        var data = new float[size * size * size * 3];
        var at = 0;
        for (var b = 0; b < size; b++)
        {
            for (var g = 0; g < size; g++)
            {
                for (var r = 0; r < size; r++)
                {
                    var (outR, outG, outB) = grade(r / (double)(size - 1), g / (double)(size - 1), b / (double)(size - 1));
                    data[at++] = (float)Clamp01(outR);
                    data[at++] = (float)Clamp01(outG);
                    data[at++] = (float)Clamp01(outB);
                }
            }
        }
        return new Lut(size, data);
    }

    private sealed record HueBand(double Hue, double Width, double Shift = 0, double Saturation = 0, double Value = 0);

    private sealed record Recipe
    {
        public double[] Gains { get; init; } = [1, 1, 1];
        public HueBand[] Bands { get; init; } = [];
        public double Saturation { get; init; } = 1;
        public double Contrast { get; init; }
        public double Fade { get; init; }
        public double Roll { get; init; }
        public double[] Shadows { get; init; } = [0, 0, 0];
        public double[] Highlights { get; init; } = [0, 0, 0];
        public double[]? Mono { get; init; }
    }

    private static (double H, double S, double V) ToHsv(double r, double g, double b)
    {
        var peak = Math.Max(r, Math.Max(g, b));
        var span = peak - Math.Min(r, Math.Min(g, b));
        if (span == 0) return (0, 0, peak);
        var sector = peak == r ? ((g - b) / span + 6) % 6 : peak == g ? (b - r) / span + 2 : (r - g) / span + 4;
        return (sector * 60, span / peak, peak);
    }

    private static double[] FromHsv(double h, double s, double v)
    {
        double Channel(double n)
        {
            var k = (n + h / 60) % 6;
            return v - v * s * Math.Max(0, Math.Min(Math.Min(k, 4 - k), 1));
        }
        return [Channel(5), Channel(3), Channel(1)];
    }

    private static double[] ApplyBands(double[] rgb, HueBand[] bands)
    {
        var (h, s, v) = ToHsv(rgb[0], rgb[1], rgb[2]);
        var (originalHue, neutral) = (h, Clamp01((s - 0.05) / 0.2));
        foreach (var band in bands)
        {
            var away = Math.Abs((((originalHue - band.Hue + 540) % 360) + 360) % 360 - 180) / band.Width;
            if (away >= 1) continue;
            var weight = (0.5 + 0.5 * Math.Cos(Math.PI * away)) * neutral;
            h += band.Shift * weight;
            s *= 1 + band.Saturation * weight;
            v *= 1 + band.Value * weight;
        }
        return FromHsv(((h + 360) % 360 + 360) % 360, Math.Min(s, 1), v);
    }

    /// <summary>Where a channel brightened by a gain starts to bend so its top still reaches 1.</summary>
    private const double GainKnee = 0.75;

    /// <summary>Multiplies by gain, rolling off above the knee so values that would pass 1 keep their order.</summary>
    private static double Gained(double value, double gain)
    {
        var scaled = value * gain;
        if (gain <= 1 || scaled <= GainKnee) return scaled;
        var t = Math.Min((scaled - GainKnee) / (gain - GainKnee), 1);
        var startSlope = (gain - GainKnee) / (1 - GainKnee);
        return GainKnee + (1 - GainKnee) * (startSlope * t + (1 - startSlope) * t * t);
    }

    private static double Ceiling(double value, double top) => Gained(value / top, top);

    private static Grade FromRecipe(Recipe recipe) => (red, green, blue) =>
    {
        double[] rgb = [Gained(red, recipe.Gains[0]), Gained(green, recipe.Gains[1]), Gained(blue, recipe.Gains[2])];
        if (recipe.Mono is { } mono)
        {
            var y = rgb[0] * mono[0] + rgb[1] * mono[1] + rgb[2] * mono[2];
            rgb = [y, y, y];
        }
        else
        {
            if (recipe.Bands.Length > 0) rgb = ApplyBands(rgb, recipe.Bands);
            var y = Luma(rgb[0], rgb[1], rgb[2]);
            rgb = rgb.Select(v => y + (v - y) * recipe.Saturation).ToArray();
        }
        var level = Clamp01(Luma(rgb[0], rgb[1], rgb[2]));
        var toned = rgb.Select((v, i) =>
            recipe.Fade + SCurve(Clamp01(v), recipe.Contrast) * (1 - recipe.Fade - recipe.Roll)
            + recipe.Shadows[i] * (1 - level) * (1 - level) + recipe.Highlights[i] * level * level).ToArray();
        return (toned[0], toned[1], toned[2]);
    };

    private static readonly (int Id, string Group, string Name, Recipe Recipe)[] Recipes =
    [
        (6, "胶片", "标准正片", new() { Contrast = 0.1, Saturation = 1.08, Bands = [new(120, 60, Shift: -6)] }),
        (7, "胶片", "鲜艳正片", new()
        {
            Contrast = 0.24, Saturation = 1.32,
            Bands = [new(120, 70, Shift: 10, Value: -0.08), new(220, 60, Saturation: 0.15, Value: -0.08), new(0, 35, Saturation: 0.1)],
        }),
        (8, "胶片", "经典反转", new()
        {
            Contrast = 0.16, Saturation = 0.78, Shadows = [-0.015, 0.01, 0.02], Highlights = [0.01, 0.005, -0.015],
            Bands = [new(10, 40, Shift: 8, Saturation: -0.1), new(210, 60, Saturation: -0.2), new(100, 60, Shift: -12, Saturation: -0.15)],
        }),
        (9, "胶片", "柔和负片", new() { Contrast = -0.06, Fade = 0.015, Saturation = 0.92, Gains = [1.01, 1, 0.99], Bands = [new(25, 35, Value: 0.03)] }),
        (10, "胶片", "人像暖调负片", new()
        {
            Contrast = 0.03, Fade = 0.025, Roll = 0.015, Saturation = 0.95, Gains = [1.03, 1, 0.95],
            Bands = [new(25, 35, Shift: -3, Saturation: -0.05, Value: 0.05), new(110, 60, Shift: -15, Saturation: -0.25)],
            Shadows = [0.012, 0, -0.008],
        }),
        (11, "胶片", "金色负片", new() { Contrast = 0.12, Saturation = 1.14, Gains = [1.05, 1, 0.9], Fade = 0.01, Bands = [new(45, 40, Saturation: 0.15)] }),
        (12, "胶片", "拍立得", new()
        {
            Contrast = -0.08, Fade = 0.07, Roll = 0.05, Saturation = 0.86, Gains = [1.03, 1, 0.94],
            Shadows = [0, 0.015, 0.03], Highlights = [0.015, 0.01, -0.02],
        }),
        (13, "电影", "低饱和电影", new()
        {
            Contrast = -0.1, Fade = 0.025, Roll = 0.02, Saturation = 0.72, Shadows = [-0.01, 0.012, 0.02], Bands = [new(110, 60, Saturation: -0.2)],
        }),
        (14, "电影", "钨丝灯夜景", new()
        {
            Contrast = 0.14, Saturation = 1.05, Gains = [0.92, 0.98, 1.1], Shadows = [-0.02, 0.005, 0.045], Highlights = [0.05, 0.005, -0.02],
            Bands = [new(0, 40, Saturation: 0.2)],
        }),
        (15, "电影", "褪银", new() { Contrast = 0.32, Saturation = 0.5, Roll = 0.01 }),
        (16, "电影", "末日橙", new()
        {
            Contrast = 0.26, Saturation = 1.05, Gains = [1.05, 0.99, 0.88], Shadows = [-0.01, 0.015, 0.03],
            Bands = [new(200, 60, Saturation: -0.35), new(100, 60, Shift: -35, Saturation: -0.4)],
        }),
        (17, "创意", "赛博朋克", new()
        {
            Contrast = 0.2, Saturation = 1.18, Gains = [1, 0.98, 1.02], Shadows = [0.015, -0.02, 0.07],
            Bands = [new(30, 55, Shift: -50, Saturation: 0.1), new(120, 65, Shift: 55), new(215, 55, Shift: -22, Saturation: 0.15), new(340, 35, Shift: -10, Saturation: 0.2)],
        }),
        (18, "创意", "霓虹夜色", new()
        {
            Contrast = 0.18, Saturation = 1.2, Gains = [0.98, 0.97, 1.05], Shadows = [0.01, -0.02, 0.08],
            Bands = [new(30, 55, Shift: -22), new(210, 65, Shift: 25, Saturation: 0.2)],
        }),
        (19, "创意", "港风复古", new()
        {
            Contrast = 0.12, Fade = 0.03, Saturation = 0.92, Shadows = [-0.015, 0.04, 0.012], Highlights = [0.04, 0.005, -0.03],
            Bands = [new(0, 35, Saturation: 0.15), new(120, 70, Shift: 18)],
        }),
        (20, "创意", "日系清新", new()
        {
            Contrast = -0.12, Fade = 0.06, Saturation = 0.86, Gains = [0.99, 1.02, 1.03], Shadows = [0, 0.015, 0.025],
            Bands = [new(110, 60, Shift: 15, Saturation: -0.1, Value: 0.06), new(25, 35, Value: 0.05)],
        }),
        (21, "创意", "交叉冲洗", new()
        {
            Contrast = 0.26, Saturation = 1.15, Shadows = [-0.02, 0.01, 0.1], Highlights = [0.03, 0.04, -0.07], Bands = [new(120, 70, Shift: -15)],
        }),
        (22, "黑白", "细腻黑白", new() { Mono = [0.25, 0.65, 0.1], Contrast = 0.1 }),
        (23, "黑白", "高反差黑白", new() { Mono = [0.4, 0.5, 0.1], Contrast = 0.34 }),
        (24, "黑白", "复古棕调", new() { Mono = [0.3, 0.6, 0.1], Contrast = 0.08, Fade = 0.04, Shadows = [0.03, 0.01, -0.03], Highlights = [0.03, 0.005, -0.05] }),
    ];

    private static readonly (LookInfo Info, Grade Grade)[] BuiltIns =
    [
        (new(1, BasicGroup, "暖调胶片"), (r, g, b) => (Ceiling(SCurve(r, 0.12) * 1.04 + 0.015, 1.055), Ceiling(SCurve(g, 0.1) + 0.005, 1.005), SCurve(b, 0.08) * 0.92 + 0.03)),
        (new(2, BasicGroup, "清冷"), (r, g, b) => (SCurve(r, 0.08) * 0.95, SCurve(g, 0.08) * 0.99, Ceiling(SCurve(b, 0.06) * 1.05 + 0.01, 1.06))),
        (new(3, BasicGroup, "青橙"), (r, g, b) =>
        {
            var warm = Luma(r, g, b);
            var cool = 1 - warm;
            return (Ceiling(r + 0.07 * warm - 0.06 * cool, 1.07), Ceiling(g + 0.015 * warm + 0.015 * cool, 1.015), Ceiling(b - 0.08 * warm + 0.07 * cool, 1.07));
        }),
        (new(4, BasicGroup, "褪色"), (r, g, b) =>
        {
            static double Lift(double v) => 0.08 + v * 0.86;
            var y = Luma(r, g, b);
            double Soften(double v) => y + (v - y) * 0.82;
            return (Lift(Soften(r)), Lift(Soften(g)), Lift(Soften(b)));
        }),
        (new(5, BasicGroup, "黑白"), (r, g, b) =>
        {
            var y = SCurve(0.3 * r + 0.6 * g + 0.1 * b, 0.15);
            return (y, y, y);
        }),
        .. Recipes.Select(look => (new LookInfo(look.Id, look.Group, look.Name), FromRecipe(look.Recipe))),
    ];

    public static IReadOnlyList<LookInfo> BuiltInLooks { get; } = BuiltIns.Select(look => look.Info).ToList();

    private static readonly Dictionary<int, Lut> Cache = [];

    public static Lut? BuiltIn(int id)
    {
        lock (Cache)
        {
            if (Cache.TryGetValue(id, out var cached)) return cached;
            var look = BuiltIns.FirstOrDefault(item => item.Info.Id == id);
            if (look.Grade is null) return null;
            return Cache[id] = Generate(look.Grade);
        }
    }

    /// <summary>Parses an Adobe/IRIDAS .cube 3D LUT. Throws with a readable reason if it cannot.</summary>
    public static Lut ParseCube(string text)
    {
        var size = 0;
        var values = new List<float>();
        foreach (var raw in text.Split('\n'))
        {
            var line = raw.Trim();
            if (line.Length == 0 || line.StartsWith('#') || line.StartsWith("TITLE", StringComparison.Ordinal)) continue;
            var parts = line.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
            if (line.StartsWith("LUT_1D_SIZE", StringComparison.Ordinal)) throw new FormatException("只支持 3D LUT，这是 1D LUT");
            if (line.StartsWith("DOMAIN_MIN", StringComparison.Ordinal) || line.StartsWith("DOMAIN_MAX", StringComparison.Ordinal))
            {
                var wanted = line.StartsWith("DOMAIN_MIN", StringComparison.Ordinal) ? 0 : 1;
                if (parts.Skip(1).Any(value => !double.TryParse(value, CultureInfo.InvariantCulture, out var number) || number != wanted))
                {
                    throw new FormatException("只支持定义域为 0–1 的 LUT");
                }
                continue;
            }
            if (line.StartsWith("LUT_3D_SIZE", StringComparison.Ordinal))
            {
                size = parts.Length > 1 && int.TryParse(parts[1], CultureInfo.InvariantCulture, out var parsed) ? parsed : 0;
                continue;
            }
            if (char.IsAsciiLetterUpper(line[0])) continue;
            if (parts.Length == 3 && parts.All(part => double.TryParse(part, CultureInfo.InvariantCulture, out var number) && double.IsFinite(number)))
            {
                values.AddRange(parts.Select(part => (float)Clamp01(double.Parse(part, CultureInfo.InvariantCulture))));
            }
        }
        if (size is < 2 or > 129) throw new FormatException("没有找到有效的 LUT_3D_SIZE");
        if (values.Count != size * size * size * 3) throw new FormatException($"LUT 数据不完整：应有 {size * size * size} 行，实际 {values.Count / 3} 行");
        return new Lut(size, [.. values]);
    }
}

public sealed record ImportedLook(int Id, string Name, string File);

/// <summary>Imported .cube files, copied into the data folder so edits keep working if the original moves.</summary>
public sealed class LookLibrary
{
    private readonly JsonFile<List<ImportedLook>> _index = new("looks.json");
    private readonly string _folder = Path.Combine(DataFolder.Path, "looks");
    private readonly List<ImportedLook> _looks;

    public LookLibrary()
    {
        _looks = _index.Load();
    }

    public IReadOnlyList<ImportedLook> Imported => _looks;

    /// <summary>Checks and copies a .cube file. Throws FormatException if it is not a usable LUT.</summary>
    public ImportedLook Import(string path)
    {
        var text = File.ReadAllText(path);
        Looks.ParseCube(text);
        Directory.CreateDirectory(_folder);
        var id = Math.Max(Looks.FirstImportedId - 1, _looks.Count == 0 ? 0 : _looks.Max(look => look.Id)) + 1;
        var file = $"{id}.cube";
        File.WriteAllText(Path.Combine(_folder, file), text);
        var look = new ImportedLook(id, Path.GetFileNameWithoutExtension(path), file);
        _looks.Add(look);
        _index.Save(_looks);
        return look;
    }

    public Lut? Resolve(int id)
    {
        if (id == 0) return null;
        if (Looks.BuiltIn(id) is { } builtIn) return builtIn;
        var look = _looks.FirstOrDefault(item => item.Id == id);
        if (look is null) return null;
        try
        {
            return Looks.ParseCube(File.ReadAllText(Path.Combine(_folder, look.File)));
        }
        catch (Exception exception) when (exception is IOException or FormatException)
        {
            return null;
        }
    }
}
