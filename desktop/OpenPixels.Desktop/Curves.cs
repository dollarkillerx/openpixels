namespace OpenPixels.Desktop;

public readonly record struct CurvePoint(double X, double Y);

/// <summary>
/// Point curves stored in an edit, and the spline the engine draws through them. Keep in
/// step with web/src/curve.ts and op-core/src/curve.rs; all three are checked against
/// crates/op-core/src/curve_cases.json.
/// </summary>
public static class Curves
{
    public static readonly string[] Channels = ["M", "R", "G", "B"];
    public static readonly IReadOnlyList<CurvePoint> Identity = [new(0, 0), new(1, 1)];

    /// <summary>Edits saved before free points stored offsets at these five fixed inputs.</summary>
    private const int LegacyPointCount = 5;
    private const double ParametricReach = 0.1;

    private static readonly (string Key, double Centre, double Width)[] ParametricRegions =
    [
        ("curveHighlights", 0.8, 0.2), ("curveLights", 0.6, 0.25), ("curveDarks", 0.4, 0.25), ("curveShadows", 0.2, 0.2),
    ];

    private static double Get(IReadOnlyDictionary<string, double> edit, string key) => edit.TryGetValue(key, out var value) ? value : 0;

    /// <summary>True for keys that hold a channel's points, in either the current or the legacy form.</summary>
    public static bool IsPointKey(string key, string channel) =>
        key.Length > 6 && key.StartsWith("curve" + channel, StringComparison.Ordinal)
        && (key[6..] is "N" || (key.Length == 7 && char.IsDigit(key[6])) || ((key[6] is 'X' or 'Y') && key[7..].All(char.IsDigit) && key.Length > 7));

    public static IReadOnlyList<CurvePoint> PointsOf(IReadOnlyDictionary<string, double> edit, string channel)
    {
        var count = (int)Get(edit, $"curve{channel}N");
        if (count > 0)
        {
            return Enumerable.Range(0, count).Select(i => new CurvePoint(Get(edit, $"curve{channel}X{i}"), Get(edit, $"curve{channel}Y{i}"))).ToList();
        }
        var legacy = Enumerable.Range(0, LegacyPointCount).Select(i => Get(edit, $"curve{channel}{i}")).ToList();
        if (legacy.All(offset => offset == 0)) return Identity;
        return legacy.Select((offset, i) => new CurvePoint(i / (double)(LegacyPointCount - 1), i / (double)(LegacyPointCount - 1) + offset)).ToList();
    }

    private static bool IsIdentity(IReadOnlyList<CurvePoint> points) =>
        points.Count == 2 && points[0] == new CurvePoint(0, 0) && points[1] == new CurvePoint(1, 1);

    /// <summary>The edit with a channel's points replaced. An untouched curve stores nothing.</summary>
    public static Dictionary<string, double> WithPoints(IReadOnlyDictionary<string, double> edit, string channel, IReadOnlyList<CurvePoint> points)
    {
        var next = edit.Where(pair => !IsPointKey(pair.Key, channel)).ToDictionary();
        if (IsIdentity(points)) return next;
        next[$"curve{channel}N"] = points.Count;
        for (var i = 0; i < points.Count; i++)
        {
            next[$"curve{channel}X{i}"] = points[i].X;
            next[$"curve{channel}Y{i}"] = points[i].Y;
        }
        return next;
    }

    public static double[] ParametricAmounts(IReadOnlyDictionary<string, double> edit) => ParametricRegions.Select(region => Get(edit, region.Key)).ToArray();

    /// <summary>How far the parametric sliders move the master curve at an input.</summary>
    public static double ParametricOffset(IReadOnlyDictionary<string, double> edit, double x)
    {
        var offset = 0.0;
        foreach (var (key, centre, width) in ParametricRegions)
        {
            var distance = Math.Abs(x - centre) / width;
            if (distance < 1) offset += Get(edit, key) * ParametricReach * (0.5 + 0.5 * Math.Cos(Math.PI * distance));
        }
        return offset;
    }

    private static double[] TangentsOf(IReadOnlyList<CurvePoint> points)
    {
        var spans = points.Count - 1;
        var widths = new double[spans];
        var slopes = new double[spans];
        for (var i = 0; i < spans; i++)
        {
            widths[i] = points[i + 1].X - points[i].X;
            slopes[i] = (points[i + 1].Y - points[i].Y) / widths[i];
        }
        var tangents = Enumerable.Repeat(slopes[0], points.Count).ToArray();
        tangents[spans] = slopes[spans - 1];
        for (var i = 1; i < spans; i++)
        {
            tangents[i] = slopes[i - 1] * slopes[i] <= 0 ? 0 : (widths[i] * slopes[i - 1] + widths[i - 1] * slopes[i]) / (widths[i - 1] + widths[i]);
        }
        if (spans > 1)
        {
            static double End(double slope, double inner) => slope * (3 * slope - inner) <= 0 ? 0 : (3 * slope - inner) / 2;
            tangents[0] = End(slopes[0], tangents[1]);
            tangents[spans] = End(slopes[spans - 1], tangents[spans - 1]);
        }
        for (var i = 0; i < spans; i++)
        {
            if (slopes[i] == 0)
            {
                tangents[i] = tangents[i + 1] = 0;
                continue;
            }
            var (a, b) = (tangents[i] / slopes[i], tangents[i + 1] / slopes[i]);
            var size = a * a + b * b;
            if (size > 9)
            {
                var scale = 3 / Math.Sqrt(size);
                tangents[i] = scale * a * slopes[i];
                tangents[i + 1] = scale * b * slopes[i];
            }
        }
        return tangents;
    }

    /// <summary>The same monotone spline the engine applies, so what is drawn is what happens.</summary>
    public static Func<double, double> SplineThrough(IReadOnlyList<CurvePoint> points)
    {
        var tangents = TangentsOf(points);
        var last = points.Count - 1;
        return x =>
        {
            if (x <= points[0].X) return points[0].Y;
            if (x >= points[last].X) return points[last].Y;
            var segment = 0;
            while (points[segment + 1].X <= x) segment++;
            var (start, end) = (points[segment], points[segment + 1]);
            var width = end.X - start.X;
            var t = (x - start.X) / width;
            var (t2, t3) = (t * t, t * t * t);
            return (2 * t3 - 3 * t2 + 1) * start.Y + (t3 - 2 * t2 + t) * width * tangents[segment] + (-2 * t3 + 3 * t2) * end.Y + (t3 - t2) * width * tangents[segment + 1];
        };
    }
}
