using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace OpenPixels.Desktop;

/// <summary>
/// Looks up lens corrections in the Lensfun database (CC BY-SA 3.0; see NOTICE.txt beside
/// web/public/lensfun/lensfun.json). Ported from web/src/lensfun.ts; the two must agree.
/// </summary>
public static partial class Lensfun
{
    private sealed record Lens(string Maker, string[] Names, double Crop, double Aspect, JsonArray Distortion, JsonArray Tca, double[][] Vignetting);

    private sealed record Database(Dictionary<string, double> Cameras, List<Lens> Lenses);

    private static readonly Dictionary<string, string[]> DistortionParameters = new() { ["ptlens"] = ["a", "b", "c"], ["poly3"] = ["k1"], ["poly5"] = ["k1", "k2"] };
    private static readonly Dictionary<string, string[]> TcaParameters = new() { ["linear"] = ["kr", "kb"], ["poly3"] = ["vr", "vb", "cr", "cb", "br", "bb"] };
    private const double CropTolerance = 1.03;

    private static readonly Lazy<Database?> Loaded = new(Load);

    /// <summary>The database file beside the app, or null if it was not bundled.</summary>
    public static string DatabasePath => Path.Combine(AppContext.BaseDirectory, "lensfun", "lensfun.json");

    private static Database? Load()
    {
        try
        {
            if (!File.Exists(DatabasePath)) return null;
            var root = JsonNode.Parse(File.ReadAllText(DatabasePath))!;
            var cameras = root["cameras"]!.AsObject().ToDictionary(pair => pair.Key, pair => pair.Value!.GetValue<double>());
            var lenses = root["lenses"]!.AsArray().Select(node => new Lens(
                node!["maker"]!.GetValue<string>(),
                node["names"]!.AsArray().Select(name => name!.GetValue<string>()).ToArray(),
                node["crop"]!.GetValue<double>(),
                node["aspect"]!.GetValue<double>(),
                node["distortion"]?.AsArray() ?? [],
                node["tca"]?.AsArray() ?? [],
                (node["vignetting"]?.AsArray() ?? []).Select(sample => sample!.AsArray().Select(value => value!.GetValue<double>()).ToArray()).ToArray())).ToList();
            return new Database(cameras, lenses);
        }
        catch (Exception exception) when (exception is IOException or JsonException or InvalidOperationException or NullReferenceException)
        {
            return null;
        }
    }

    private static string MakerFamily(string maker) =>
        maker.StartsWith("om digital", StringComparison.OrdinalIgnoreCase) ? "olympus" : maker.ToLowerInvariant().Split(' ')[0];

    [GeneratedRegex(@"\b(sigma|tamron|tokina|samyang|rokinon|viltrox|canon|nikon|sony|fujifilm|fujinon|panasonic|olympus)\b")]
    private static partial Regex Makers();
    [GeneratedRegex(@"[|\[(]\s*(a|art)\s*\d*\s*[\])]?\s*$")]
    private static partial Regex ArtLine();
    [GeneratedRegex(@"[|\[(]\s*(c|contemporary)\s*\d*\s*[\])]?\s*$")]
    private static partial Regex ContemporaryLine();
    [GeneratedRegex(@"[|\[(]\s*(s|sports)\s*\d*\s*[\])]?\s*$")]
    private static partial Regex SportsLine();
    [GeneratedRegex(@"\(?\b[a-z]\d{3}\)?\s*$")]
    private static partial Regex ModelCode();
    [GeneratedRegex(@"[^a-z0-9.]")]
    private static partial Regex NotNameCharacter();

    /// <summary>Must match normalize in scripts/build-lensfun.mjs and web/src/lensfun.ts.</summary>
    public static string Normalize(string text)
    {
        var name = Makers().Replace(text.ToLowerInvariant(), "");
        name = ArtLine().Replace(name, "a");
        name = ContemporaryLine().Replace(name, "c");
        name = SportsLine().Replace(name, "s");
        name = ModelCode().Replace(name, "");
        return NotNameCharacter().Replace(name, "");
    }

    private static (double Low, double High, double T) Bracket(IReadOnlyList<double> values, double at)
    {
        if (at <= values[0]) return (values[0], values[0], 0);
        var last = values[^1];
        if (at >= last) return (last, last, 0);
        var upper = values.ToList().FindIndex(value => value >= at);
        var (low, high) = (values[upper - 1], values[upper]);
        return (low, high, (at - low) / (high - low));
    }

    private static List<double> Unique(IEnumerable<double> values) => values.Distinct().Order().ToList();

    private static JsonObject? AtFocal(JsonArray samples, double focal, Dictionary<string, string[]> parameters)
    {
        if (samples.Count == 0) return null;
        static double Focal(JsonNode? sample) => sample![0]!.GetValue<double>();
        static string Model(JsonNode? sample) => sample![1]!.GetValue<string>();
        var nearest = samples.Aggregate((best, sample) => Math.Abs(Focal(sample) - focal) < Math.Abs(Focal(best) - focal) ? sample : best);
        var model = Model(nearest);
        if (!parameters.TryGetValue(model, out var names)) return null;
        var same = samples.Where(sample => Model(sample) == model).ToList();
        var (low, high, t) = Bracket(Unique(same.Select(Focal)), focal);
        var lower = same.First(sample => Focal(sample) == low)!;
        var upper = same.First(sample => Focal(sample) == high)!;
        var result = new JsonObject { ["model"] = model };
        for (var index = 0; index < names.Length; index++)
        {
            var a = lower[index + 2]!.GetValue<double>();
            result[names[index]] = a + (upper[index + 2]!.GetValue<double>() - a) * t;
        }
        return result;
    }

    private static JsonObject? VignettingAt(double[][] samples, double focal, double aperture)
    {
        if (samples.Length == 0) return null;
        var farthest = new Dictionary<(double, double), double[]>();
        foreach (var sample in samples)
        {
            var key = (sample[0], sample[1]);
            if (!farthest.TryGetValue(key, out var known) || sample[2] > known[2]) farthest[key] = sample;
        }
        var usable = farthest.Values.ToList();
        double[] AtFocalLength(double value)
        {
            var here = usable.Where(sample => sample[0] == value).ToList();
            // Falloff changes about evenly per stop, so interpolate in stops.
            var (low, high, t) = Bracket(Unique(here.Select(sample => Math.Log2(sample[1]))), Math.Log2(aperture));
            var a = here.First(sample => Math.Log2(sample[1]) == low);
            var b = here.First(sample => Math.Log2(sample[1]) == high);
            return new[] { 3, 4, 5 }.Select(index => a[index] + (b[index] - a[index]) * t).ToArray();
        }
        var (lowFocal, highFocal, along) = Bracket(Unique(usable.Select(sample => sample[0])), focal);
        var (first, second) = (AtFocalLength(lowFocal), AtFocalLength(highFocal));
        var k = first.Select((value, index) => value + (second[index] - value) * along).ToArray();
        return new JsonObject { ["k1"] = k[0], ["k2"] = k[1], ["k3"] = k[2] };
    }

    /// <summary>
    /// The corrections for a shot as op-core's LensfunCalibration JSON, or null if the database
    /// is missing or does not know the lens. `query` is the lensQuery from decode.
    /// </summary>
    public static string? Calibration(JsonNode? query)
    {
        if (Loaded.Value is not { } db || query is null) return null;
        var lensName = query["lens"]?.GetValue<string>();
        var cameraName = query["camera"]?.GetValue<string>() ?? "";
        var focal = query["focal"]?.GetValue<double>() ?? 0;
        var aperture = query["aperture"]?.GetValue<double?>();
        if (string.IsNullOrEmpty(lensName)) return null;

        // Exact names only: a near match would silently apply another lens's corrections.
        var name = Normalize(lensName);
        var entries = db.Lenses.Where(entry => entry.Names.Contains(name)).ToList();
        // With the maker's name gone, one name could mean lenses from two makers.
        if (entries.Count == 0 || entries.Select(entry => MakerFamily(entry.Maker)).Distinct().Count() > 1) return null;
        // A lens can be calibrated on several sensors. One smaller than this camera's would
        // leave the corners unmeasured, so use the largest that is not.
        var crop = db.Cameras.TryGetValue(Normalize(cameraName), out var known) ? known : entries.Min(entry => entry.Crop);
        var lens = entries.Where(entry => entry.Crop <= crop * CropTolerance).OrderByDescending(entry => entry.Crop).FirstOrDefault();
        if (lens is null) return null;

        var distortion = AtFocal(lens.Distortion, focal, DistortionParameters);
        var tca = AtFocal(lens.Tca, focal, TcaParameters);
        var vignetting = aperture is > 0 ? VignettingAt(lens.Vignetting, focal, aperture.Value) : null;
        if (distortion is null && tca is null && vignetting is null) return null;
        var relative = lens.Crop / crop;
        return new JsonObject
        {
            ["distortion"] = distortion,
            ["tca"] = tca,
            ["vignetting"] = vignetting,
            ["huginScale"] = relative * Math.Sqrt(lens.Aspect * lens.Aspect + 1),
            ["cornerScale"] = relative,
        }.ToJsonString();
    }
}
