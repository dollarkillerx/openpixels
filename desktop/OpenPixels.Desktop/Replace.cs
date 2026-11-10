using Avalonia;
using Avalonia.Media.Imaging;
using Avalonia.Platform;

namespace OpenPixels.Desktop;

/// <summary>An image to put behind the subject or in place of the sky, as straight RGBA.</summary>
public sealed record ReplaceImage(int Width, int Height, byte[] Rgba);

public sealed record ReplaceChoice(int Id, string Name)
{
    public override string ToString() => Name;
}

public sealed record ImportedReplacement(int Id, string Name, bool Sky, string File);

/// <summary>
/// Backgrounds and skies. The built-in ones are drawn here, as in web/src/replace.ts, so the
/// project ships no third-party imagery; ids match the web edition.
/// </summary>
public static class Replacements
{
    public const int FirstImportedId = 100;
    private const int Width = 1600;
    private const int Height = 1067;
    /// <summary>Imported images are kept at this size at most.</summary>
    private const int MaxLongEdge = 4096;

    private delegate void Painter(double[] rgb);

    private static (double R, double G, double B) Hex(string colour) =>
        (Convert.ToInt32(colour[1..3], 16), Convert.ToInt32(colour[3..5], 16), Convert.ToInt32(colour[5..7], 16));

    private static Painter Flat(string colour) => rgb =>
    {
        var (r, g, b) = Hex(colour);
        for (var at = 0; at < rgb.Length; at += 3) (rgb[at], rgb[at + 1], rgb[at + 2]) = (r, g, b);
    };

    private static Painter Vertical(params (double At, string Colour)[] stops) => rgb =>
    {
        for (var y = 0; y < Height; y++)
        {
            var t = y / (double)Height;
            var upper = Array.FindIndex(stops, stop => stop.At >= t);
            var (r, g, b) = upper < 0 ? Hex(stops[^1].Colour)
                : upper == 0 ? Hex(stops[0].Colour)
                : Mix(Hex(stops[upper - 1].Colour), Hex(stops[upper].Colour), (t - stops[upper - 1].At) / (stops[upper].At - stops[upper - 1].At));
            for (var x = 0; x < Width; x++) (rgb[(y * Width + x) * 3], rgb[(y * Width + x) * 3 + 1], rgb[(y * Width + x) * 3 + 2]) = (r, g, b);
        }
    };

    private static (double, double, double) Mix((double R, double G, double B) a, (double R, double G, double B) b, double t) =>
        (a.R + (b.R - a.R) * t, a.G + (b.G - a.G) * t, a.B + (b.B - a.B) * t);

    private static readonly Painter Studio = rgb =>
    {
        var (inner, outer) = (Hex("#8f8f92"), Hex("#37373a"));
        var (cx, cy, radius) = (Width / 2.0, Height * 0.45, Width * 0.7);
        for (var y = 0; y < Height; y++)
        {
            for (var x = 0; x < Width; x++)
            {
                var t = Math.Min(1, Math.Sqrt((x - cx) * (x - cx) + (y - cy) * (y - cy)) / radius);
                var (r, g, b) = Mix(inner, outer, t);
                (rgb[(y * Width + x) * 3], rgb[(y * Width + x) * 3 + 1], rgb[(y * Width + x) * 3 + 2]) = (r, g, b);
            }
        }
    };

    private static Func<double, double, double> ValueNoise(int seed)
    {
        var lattice = Enumerable.Range(0, 256).Select(index =>
        {
            var value = Math.Sin((index + seed) * 127.1) * 43758.5453;
            return value - Math.Floor(value);
        }).ToArray();
        double At(int x, int y) => lattice[(x * 57 + y * 131) & 255];
        static double Smooth(double t) => t * t * (3 - 2 * t);
        return (x, y) =>
        {
            var (ix, iy) = ((int)Math.Floor(x), (int)Math.Floor(y));
            var (tx, ty) = (Smooth(x - ix), Smooth(y - iy));
            var top = At(ix, iy) * (1 - tx) + At(ix + 1, iy) * tx;
            var bottom = At(ix, iy + 1) * (1 - tx) + At(ix + 1, iy + 1) * tx;
            return top * (1 - ty) + bottom * ty;
        };
    }

    /// <summary>Soft clouds of a colour over what is already drawn.</summary>
    private static Painter Clouds((double R, double G, double B) colour, double cover) => rgb =>
    {
        var noise = ValueNoise(11);
        Parallel.For(0, Height, y =>
        {
            for (var x = 0; x < Width; x++)
            {
                double value = 0, amplitude = 0.5, fx = x / 260.0, fy = y / 110.0;
                // Wider than tall, as clouds look when seen from below at a distance.
                for (var octave = 0; octave < 5; octave++, fx *= 2, fy *= 2, amplitude /= 2) value += noise(fx, fy) * amplitude;
                var t = Math.Min(1, Math.Max(0, (value - cover) / 0.22));
                var alpha = t * t * (3 - 2 * t) * 0.92;
                var at = (y * Width + x) * 3;
                rgb[at] += (colour.R - rgb[at]) * alpha;
                rgb[at + 1] += (colour.G - rgb[at + 1]) * alpha;
                rgb[at + 2] += (colour.B - rgb[at + 2]) * alpha;
            }
        });
    };

    private static Painter Both(params Painter[] painters) => rgb =>
    {
        foreach (var paint in painters) paint(rgb);
    };

    private static readonly (int Id, string Name, bool Sky, Painter Paint)[] BuiltIns =
    [
        (1, I18n.T("纯白"), false, Flat("#f4f4f4")),
        (2, I18n.T("中灰"), false, Flat("#808082")),
        (3, I18n.T("纯黑"), false, Flat("#101012")),
        (4, I18n.T("影棚灰"), false, Studio),
        (11, I18n.T("晴空"), true, Both(Vertical((0, "#2f6fc0"), (0.75, "#8db8e4"), (1, "#d4e4f2")), Clouds((250, 250, 252), 0.56))),
        (12, I18n.T("晚霞"), true, Both(Vertical((0, "#2c3c74"), (0.5, "#c06a78"), (0.85, "#f4a65e"), (1, "#fbd59a")), Clouds((255, 196, 150), 0.6))),
        (13, I18n.T("阴天"), true, Both(Vertical((0, "#7f8791"), (1, "#c3c8ce")), Clouds((214, 217, 222), 0.42))),
    ];

    private static readonly Dictionary<int, ReplaceImage> Painted = [];

    public static IEnumerable<ReplaceChoice> BuiltInChoices(bool sky) =>
        BuiltIns.Where(item => item.Sky == sky).Select(item => new ReplaceChoice(item.Id, item.Name));

    public static ReplaceImage? BuiltIn(int id)
    {
        lock (Painted)
        {
            if (Painted.TryGetValue(id, out var cached)) return cached;
            var item = BuiltIns.FirstOrDefault(entry => entry.Id == id);
            if (item.Paint is null) return null;
            var rgb = new double[Width * Height * 3];
            item.Paint(rgb);
            var rgba = new byte[Width * Height * 4];
            for (var i = 0; i < Width * Height; i++)
            {
                for (var channel = 0; channel < 3; channel++) rgba[i * 4 + channel] = (byte)Math.Clamp(Math.Round(rgb[i * 3 + channel]), 0, 255);
                rgba[i * 4 + 3] = 255;
            }
            return Painted[id] = new ReplaceImage(Width, Height, rgba);
        }
    }

    /// <summary>Reads a PNG or JPEG as straight RGBA, at most MaxLongEdge on its long side.</summary>
    public static ReplaceImage Decode(string path)
    {
        using var source = new Bitmap(path);
        var scale = Math.Min(1, MaxLongEdge / (double)Math.Max(source.PixelSize.Width, source.PixelSize.Height));
        var size = new PixelSize(Math.Max(1, (int)Math.Round(source.PixelSize.Width * scale)), Math.Max(1, (int)Math.Round(source.PixelSize.Height * scale)));
        using var scaled = scale < 1 ? source.CreateScaledBitmap(size) : null;
        var bitmap = scaled ?? source;
        var (width, height) = (bitmap.PixelSize.Width, bitmap.PixelSize.Height);
        var pixels = new byte[width * height * 4];
        unsafe
        {
            fixed (byte* buffer = pixels) bitmap.CopyPixels(new PixelRect(0, 0, width, height), (nint)buffer, pixels.Length, width * 4);
        }
        // Decoded bitmaps are usually BGRA; the engine wants RGBA, and photos are opaque.
        if (bitmap.Format != PixelFormat.Rgba8888)
        {
            for (var at = 0; at < pixels.Length; at += 4) (pixels[at], pixels[at + 2]) = (pixels[at + 2], pixels[at]);
        }
        for (var at = 3; at < pixels.Length; at += 4) pixels[at] = 255;
        return new ReplaceImage(width, height, pixels);
    }
}

/// <summary>Backgrounds and skies the user imported, copied into the data folder.</summary>
public sealed class ReplacementLibrary
{
    private readonly JsonFile<List<ImportedReplacement>> _index = new("replacements.json");
    private readonly string _folder = Path.Combine(DataFolder.Path, "replacements");
    private readonly List<ImportedReplacement> _items;

    public ReplacementLibrary()
    {
        _items = _index.Load();
    }

    public IEnumerable<ReplaceChoice> Choices(bool sky) => _items.Where(item => item.Sky == sky).Select(item => new ReplaceChoice(item.Id, item.Name));

    /// <summary>Checks and copies an image. Throws if it cannot be read.</summary>
    public ImportedReplacement Import(string path, bool sky)
    {
        Replacements.Decode(path);
        Directory.CreateDirectory(_folder);
        var id = Math.Max(Replacements.FirstImportedId - 1, _items.Count == 0 ? 0 : _items.Max(item => item.Id)) + 1;
        var file = $"{id}{Path.GetExtension(path).ToLowerInvariant()}";
        File.Copy(path, Path.Combine(_folder, file), overwrite: true);
        var item = new ImportedReplacement(id, Path.GetFileNameWithoutExtension(path), sky, file);
        _items.Add(item);
        _index.Save(_items);
        return item;
    }

    public ReplaceImage? Resolve(int id)
    {
        if (id <= 0) return null;
        if (Replacements.BuiltIn(id) is { } builtIn) return builtIn;
        var item = _items.FirstOrDefault(entry => entry.Id == id);
        if (item is null) return null;
        try
        {
            return Replacements.Decode(Path.Combine(_folder, item.File));
        }
        catch (Exception exception) when (exception is IOException or ArgumentException or NotSupportedException)
        {
            return null;
        }
    }
}

/// <summary>
/// Builds a look that gives one image the tonal distribution and colour balance of another.
/// Statistical, with no model involved. Ported from web/src/colormatch.ts.
/// </summary>
public static class ColourMatch
{
    private const int LutSize = 33;
    private const int LuminancePoints = 33;
    private const int ChromaBands = 5;
    private static readonly double[] ChromaGainLimits = [0.6, 1.5];
    private const double MaxChromaShift = 40;
    private const double ToneStrength = 0.85;
    private static readonly double[] UsableL = [1, 99];
    private static readonly double[] White = [0.95047, 1, 1.08883];

    private static double ToLinear(double v) => v <= 0.04045 ? v / 12.92 : Math.Pow((v + 0.055) / 1.055, 2.4);
    private static double ToSrgb(double v) => v <= 0.0031308 ? v * 12.92 : 1.055 * Math.Pow(v, 1 / 2.4) - 0.055;
    private static readonly double[] LinearByte = Enumerable.Range(0, 256).Select(i => ToLinear(i / 255.0)).ToArray();
    private static double F(double t) => t > 216.0 / 24389 ? Math.Cbrt(t) : 841.0 / 108 * t + 4.0 / 29;
    private static double FInverse(double t) => t > 6.0 / 29 ? t * t * t : 108.0 / 841 * (t - 4.0 / 29);

    private static (double L, double A, double B) LinearToLab(double r, double g, double b)
    {
        var x = F((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / White[0]);
        var y = F(0.2126729 * r + 0.7151522 * g + 0.072175 * b);
        var z = F((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / White[2]);
        return (116 * y - 16, 500 * (x - y), 200 * (y - z));
    }

    private static double[] LabToLinear(double l, double a, double b)
    {
        var y = (l + 16) / 116;
        var (x, yy, z) = (White[0] * FInverse(y + a / 500), FInverse(y), White[2] * FInverse(y - b / 200));
        return [3.2404542 * x - 1.5371385 * yy - 0.4985314 * z, -0.969266 * x + 1.8760108 * yy + 0.041556 * z, 0.0556434 * x - 0.2040259 * yy + 1.0572252 * z];
    }

    private sealed record Statistics(double[] Quantiles, (double A, double B)[] Mean, (double A, double B) Deviation);

    private static double BandWeight(double l, int band) => Math.Max(0, 1 - Math.Abs(l - band / (double)(ChromaBands - 1) * 100) / (100.0 / (ChromaBands - 1)));

    private static Statistics Measure(byte[] rgb, int step)
    {
        var lightness = new List<double>();
        var total = new double[ChromaBands, 3];
        double sumA = 0, sumB = 0, sumAA = 0, sumBB = 0;
        for (var at = 0; at + 2 < rgb.Length; at += step)
        {
            var (l, a, b) = LinearToLab(LinearByte[rgb[at]], LinearByte[rgb[at + 1]], LinearByte[rgb[at + 2]]);
            if (l < UsableL[0] || l > UsableL[1]) continue;
            lightness.Add(l);
            (sumA, sumB, sumAA, sumBB) = (sumA + a, sumB + b, sumAA + a * a, sumBB + b * b);
            for (var band = 0; band < ChromaBands; band++)
            {
                var weight = BandWeight(l, band);
                total[band, 0] += a * weight;
                total[band, 1] += b * weight;
                total[band, 2] += weight;
            }
        }
        var count = lightness.Count;
        if (count == 0) throw new FormatException(I18n.T("这张图几乎全是纯黑或纯白，无法分析色彩"));
        lightness.Sort();
        var quantiles = Enumerable.Range(0, LuminancePoints).Select(i => lightness[Math.Min(count - 1, (int)Math.Round(i / (double)(LuminancePoints - 1) * (count - 1), MidpointRounding.AwayFromZero))]).ToArray();
        var (meanA, meanB) = (sumA / count, sumB / count);
        double Spread(double sum, double mean) => Math.Sqrt(Math.Max(1e-6, sum / count - mean * mean));
        // A band with almost no pixels falls back to the overall balance.
        var means = Enumerable.Range(0, ChromaBands).Select(band => total[band, 2] > count * 0.01 ? (total[band, 0] / total[band, 2], total[band, 1] / total[band, 2]) : (meanA, meanB)).ToArray();
        return new Statistics(quantiles, means, (Spread(sumAA, meanA), Spread(sumBB, meanB)));
    }

    private static Func<double, double> ToneCurve(double[] from, double[] to)
    {
        double[] xs = [0, .. from, 100];
        double[] ys = [0, .. to, 100];
        for (var i = 1; i < xs.Length; i++)
        {
            xs[i] = Math.Max(xs[i], xs[i - 1] + 1e-3);
            ys[i] = Math.Max(ys[i], ys[i - 1]);
        }
        return l =>
        {
            var i = 1;
            while (i < xs.Length - 1 && xs[i] < l) i++;
            var t = Math.Clamp((l - xs[i - 1]) / (xs[i] - xs[i - 1]), 0, 1);
            return l + (ys[i - 1] + (ys[i] - ys[i - 1]) * t - l) * ToneStrength;
        };
    }

    private static double BandMean((double A, double B)[] mean, double l, int channel)
    {
        double sum = 0, weights = 0;
        for (var band = 0; band < ChromaBands; band++)
        {
            var weight = BandWeight(l, band);
            sum += (channel == 0 ? mean[band].A : mean[band].B) * weight;
            weights += weight;
        }
        return weights > 0 ? sum / weights : 0;
    }

    /// <summary>A look that makes source resemble reference. Both are packed pixels with the given byte steps.</summary>
    public static Lut Match(byte[] source, int sourceStep, byte[] reference, int referenceStep)
    {
        var from = Measure(source, sourceStep);
        var to = Measure(reference, referenceStep);
        var curve = ToneCurve(from.Quantiles, to.Quantiles);
        double[] gain =
        [
            Math.Clamp(to.Deviation.A / from.Deviation.A, ChromaGainLimits[0], ChromaGainLimits[1]),
            Math.Clamp(to.Deviation.B / from.Deviation.B, ChromaGainLimits[0], ChromaGainLimits[1]),
        ];
        var data = new float[LutSize * LutSize * LutSize * 3];
        var at = 0;
        for (var b = 0; b < LutSize; b++)
        {
            for (var g = 0; g < LutSize; g++)
            {
                for (var r = 0; r < LutSize; r++)
                {
                    var (l, labA, labB) = LinearToLab(ToLinear(r / (double)(LutSize - 1)), ToLinear(g / (double)(LutSize - 1)), ToLinear(b / (double)(LutSize - 1)));
                    var outL = Math.Clamp(curve(l), 0, 100);
                    double Move(double value, int channel)
                    {
                        var sourceMean = BandMean(from.Mean, l, channel);
                        var shift = Math.Clamp(BandMean(to.Mean, outL, channel) - sourceMean, -MaxChromaShift, MaxChromaShift);
                        return sourceMean + shift + (value - sourceMean) * gain[channel];
                    }
                    foreach (var value in LabToLinear(outL, Move(labA, 0), Move(labB, 1))) data[at++] = (float)Math.Clamp(ToSrgb(Math.Clamp(value, 0, 1)), 0, 1);
                }
            }
        }
        return new Lut(LutSize, data);
    }

    /// <summary>A LUT as .cube text, so a matched look is kept like an imported one.</summary>
    public static string ToCube(Lut lut, string title)
    {
        var text = new System.Text.StringBuilder();
        text.Append("TITLE \"").Append(title.Replace("\"", "'")).Append("\"\nLUT_3D_SIZE ").Append(lut.Size).Append('\n');
        for (var i = 0; i < lut.Data.Length; i += 3)
        {
            text.Append(System.FormattableString.Invariant($"{lut.Data[i]:0.######} {lut.Data[i + 1]:0.######} {lut.Data[i + 2]:0.######}\n"));
        }
        return text.ToString();
    }
}
