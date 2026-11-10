namespace OpenPixels.Desktop;

/// <summary>What culling needs to know about a photo. Field names follow web/src/analysis.ts.</summary>
public sealed record PhotoAnalysis(double Time, bool TimeKnown, double Sharpness, double Clipped, int Faces, bool EyesClosed, double[] Signature);

public sealed record Verdict(int Score, bool Best, bool Reject);

/// <summary>
/// Scores photos for culling and groups the ones taken of the same moment. Ported from
/// web/src/analysis.ts; the desktop edition has no face finder yet, so sharpness is judged
/// on the middle of the frame and closed eyes are not detected.
/// </summary>
public static class Analysis
{
    private const int SignatureGrid = 8;
    private const double CentreMargin = 0.2;
    private const double MaxGroupGapMs = 2 * 60 * 1000;
    private const double SameSceneTimed = 0.05;
    private const double SameSceneUntimed = 0.03;
    private const double SoftVarianceRatio = 3;
    private const double BadlyExposedExtra = 0.15;

    private static double Luma(byte[] rgb, int at) => 0.2126 * rgb[at] + 0.7152 * rgb[at + 1] + 0.0722 * rgb[at + 2];

    private static double LaplacianVariance(byte[] rgb, int width, int left, int top, int right, int bottom)
    {
        double sum = 0, squares = 0;
        var count = 0;
        for (var y = Math.Max(1, top); y < bottom - 1; y++)
        {
            for (var x = Math.Max(1, left); x < right - 1; x++)
            {
                var at = (y * width + x) * 3;
                var value = Luma(rgb, at - 3) + Luma(rgb, at + 3) + Luma(rgb, at - width * 3) + Luma(rgb, at + width * 3) - 4 * Luma(rgb, at);
                sum += value;
                squares += value * value;
                count++;
            }
        }
        return count > 0 ? squares / count - Math.Pow(sum / count, 2) : 0;
    }

    public static PhotoAnalysis Analyze(byte[] rgb, int width, int height, double time, bool timeKnown)
    {
        var clippedPixels = 0;
        var cells = new double[SignatureGrid * SignatureGrid * 3];
        var counts = new int[SignatureGrid * SignatureGrid];
        for (var y = 0; y < height; y++)
        {
            var row = Math.Min(SignatureGrid - 1, y * SignatureGrid / height);
            for (var x = 0; x < width; x++)
            {
                var at = (y * width + x) * 3;
                var (r, g, b) = (rgb[at], rgb[at + 1], rgb[at + 2]);
                var peak = Math.Max(r, Math.Max(g, b));
                if (peak >= 254 || peak <= 2) clippedPixels++;
                var cell = row * SignatureGrid + Math.Min(SignatureGrid - 1, x * SignatureGrid / width);
                // Brightness and two colour differences: small shifts in framing change these little.
                cells[cell * 3] += (0.3 * r + 0.6 * g + 0.1 * b) / 255;
                cells[cell * 3 + 1] += (r - g) / 510.0 + 0.5;
                cells[cell * 3 + 2] += (b - g) / 510.0 + 0.5;
                counts[cell]++;
            }
        }
        var signature = cells.Select((value, index) => Math.Round(value / Math.Max(1, counts[index / 3]) * 1000) / 1000).ToArray();
        var sharpness = LaplacianVariance(rgb, width, (int)Math.Round(width * CentreMargin), (int)Math.Round(height * CentreMargin),
            (int)Math.Round(width * (1 - CentreMargin)), (int)Math.Round(height * (1 - CentreMargin)));
        return new PhotoAnalysis(time, timeKnown, sharpness, clippedPixels / (double)(width * height), 0, false, signature);
    }

    public static double SignatureDistance(double[] a, double[] b)
    {
        var total = 0.0;
        for (var i = 0; i < a.Length; i++) total += Math.Abs(a[i] - b[i]);
        return total / a.Length;
    }

    /// <summary>Splits photos, in shooting order, wherever the moment or the scene changes.</summary>
    public static List<List<string>> Group(IEnumerable<(string Id, PhotoAnalysis Analysis)> items)
    {
        var ordered = items.OrderBy(item => item.Analysis.Time).ThenBy(item => item.Id, StringComparer.Ordinal).ToList();
        var groups = new List<List<(string Id, PhotoAnalysis Analysis)>>();
        foreach (var item in ordered)
        {
            var group = groups.LastOrDefault();
            var previous = group?[^1];
            var timed = previous is { } before && before.Analysis.TimeKnown && item.Analysis.TimeKnown;
            var tooLate = previous is { } last && timed && item.Analysis.Time - last.Analysis.Time > MaxGroupGapMs;
            if (previous is { } prior && !tooLate && SignatureDistance(prior.Analysis.Signature, item.Analysis.Signature) <= (timed ? SameSceneTimed : SameSceneUntimed))
            {
                group!.Add(item);
            }
            else
            {
                groups.Add([item]);
            }
        }
        return groups.Select(group => group.Select(item => item.Id).ToList()).ToList();
    }

    /// <summary>A photo's score in 0..100, relative to the sharpest one it is compared with.</summary>
    public static int Score(PhotoAnalysis analysis, double bestSharpness)
    {
        // On a log scale, so "half as sharp" costs the same at any level of detail.
        var relative = bestSharpness > 0 ? Math.Max(0, 1 + Math.Log(Math.Max(analysis.Sharpness, 1e-6) / bestSharpness) / Math.Log(8)) : 1;
        var exposure = Math.Max(0, 1 - analysis.Clipped * 4);
        var eyes = analysis.EyesClosed ? 0 : 1;
        return (int)Math.Round(100 * (0.55 * relative + 0.2 * exposure + 0.25 * eyes), MidpointRounding.AwayFromZero);
    }

    /// <summary>Ranks one group. A photo on its own is its group's best, and never rejected.</summary>
    public static Dictionary<string, Verdict> Judge(IReadOnlyList<(string Id, PhotoAnalysis Analysis)> group)
    {
        var bestSharpness = group.Max(item => item.Analysis.Sharpness);
        var scored = group.Select(item => (item, score: Score(item.Analysis, bestSharpness))).ToList();
        var winner = scored.Aggregate((best, entry) => entry.score > best.score ? entry : best);
        var verdicts = new Dictionary<string, Verdict>();
        foreach (var (item, score) in scored)
        {
            var best = item.Id == winner.item.Id;
            var analysis = item.Analysis;
            var reject = !best
                && ((analysis.EyesClosed && !winner.item.Analysis.EyesClosed)
                    || analysis.Sharpness * SoftVarianceRatio < bestSharpness
                    || analysis.Clipped > winner.item.Analysis.Clipped + BadlyExposedExtra);
            verdicts[item.Id] = new Verdict(score, best, reject);
        }
        return verdicts;
    }

    /// <summary>Milliseconds for a "YYYY:MM:DD HH:MM:SS" capture time, or null. Only differences matter.</summary>
    public static double? CaptureMillis(string? text)
    {
        if (text is null || text.Length < 19) return null;
        return DateTime.TryParseExact(text[..19], "yyyy:MM:dd HH:mm:ss", System.Globalization.CultureInfo.InvariantCulture,
            System.Globalization.DateTimeStyles.AssumeUniversal | System.Globalization.DateTimeStyles.AdjustToUniversal, out var time)
            ? (time - DateTime.UnixEpoch).TotalMilliseconds
            : null;
    }
}
