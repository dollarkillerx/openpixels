using System.Text.RegularExpressions;
using Avalonia;
using Avalonia.Media;
using Avalonia.Media.Imaging;
using Avalonia.Platform;

namespace OpenPixels.Desktop;

public sealed class WatermarkSettings
{
    public bool Enabled { get; set; }
    public string Text { get; set; } = "";
    /// <summary>A PNG or JPEG drawn before the text, or null for text alone.</summary>
    public string? LogoPath { get; set; }
    /// <summary>Text height as a fraction of the long edge.</summary>
    public double Size { get; set; } = 0.03;
    public double Opacity { get; set; } = 0.7;
    public string Position { get; set; } = "bottomRight";

    public WatermarkSettings Copy() => (WatermarkSettings)MemberwiseClone();
}

/// <summary>How finished files are written. Field names match the web edition's OutputSettings.</summary>
public sealed class OutputSettings
{
    public string Format { get; set; } = "jpeg";
    public string ColorSpace { get; set; } = "srgb";
    public int Quality { get; set; } = 92;
    /// <summary>Bits per channel for PNG and TIFF; JPEG is always 8.</summary>
    public int BitDepth { get; set; } = 8;
    /// <summary>Longest side in pixels; 0 keeps the full size.</summary>
    public int LongEdge { get; set; }
    public string Template { get; set; } = "{name}";
    public WatermarkSettings Watermark { get; set; } = new();

    public OutputSettings Copy()
    {
        var copy = (OutputSettings)MemberwiseClone();
        copy.Watermark = Watermark.Copy();
        return copy;
    }

    public string Extension => Format switch { "png" => "png", "tiff" => "tif", _ => "jpg" };

    /// <summary>What the engine's exportImage takes as settings.</summary>
    public object EngineSettings => new { format = Format, quality = Quality, longEdge = LongEdge, bitDepth = Format == "jpeg" ? 8 : BitDepth, colorSpace = ColorSpace };
}

public sealed record OverlayRaster(byte[] Rgba, int X, int Y, int Width, int Height);

public static partial class Output
{
    public static readonly (string Value, string Label)[] Formats = [("jpeg", "JPEG"), ("png", "PNG"), ("tiff", "TIFF")];
    public static readonly (string Value, string Label)[] ColorSpaces = [("srgb", "sRGB"), ("p3", "Display P3"), ("adobe", "Adobe RGB"), ("prophoto", "ProPhoto RGB")];
    public static readonly int[] LongEdges = [0, 6000, 4096, 3000, 2048, 1600, 1080];
    public static readonly (string Value, string Label)[] Positions =
        [("bottomRight", I18n.T("右下")), ("bottomLeft", I18n.T("左下")), ("topRight", I18n.T("右上")), ("topLeft", I18n.T("左上")), ("center", I18n.T("居中"))];

    private const double Margin = 0.02;
    private const double LogoHeight = 2;
    private const double Gap = 0.5;

    [GeneratedRegex(@"[\\/:*?""<>|]")]
    private static partial Regex ForbiddenInNames();

    /// <summary>Output name from a template, made unique among names already in used. As in the web edition.</summary>
    public static string ExportName(string sourceName, ISet<string> used, string template, int index, string extension)
    {
        var stem = Path.GetFileNameWithoutExtension(sourceName);
        if (stem.Length == 0) stem = "photo";
        var filled = ForbiddenInNames().Replace(
            template.Replace("{name}", stem).Replace("{n}", index.ToString("000")).Replace("{date}", DateTime.Now.ToString("yyyyMMdd")), "_").Trim();
        var stemOut = filled.Length > 0 ? filled : stem;
        var name = $"{stemOut}.{extension}";
        for (var copy = 2; used.Contains(name.ToLowerInvariant()); copy++) name = $"{stemOut}-{copy}.{extension}";
        used.Add(name.ToLowerInvariant());
        return name;
    }

    /// <summary>Draws a watermark for an image of this size, or returns null if there is none. Call on the UI thread.</summary>
    public static OverlayRaster? RenderWatermark(WatermarkSettings mark, int imageWidth, int imageHeight)
    {
        var text = mark.Text.Trim();
        Bitmap? logo = null;
        if (mark.Enabled && mark.LogoPath is { Length: > 0 } path && File.Exists(path))
        {
            try
            {
                logo = new Bitmap(path);
            }
            catch (Exception exception) when (exception is IOException or ArgumentException or NotSupportedException)
            {
                logo = null;
            }
        }
        if (!mark.Enabled || (text.Length == 0 && logo is null)) return null;

        var longEdge = Math.Max(imageWidth, imageHeight);
        var fontSize = Math.Max(1, Math.Round(mark.Size * longEdge));
        var margin = (int)Math.Round(Margin * longEdge);
        var typeface = new Typeface(FontFamily.Default, FontStyle.Normal, FontWeight.SemiBold);
        FormattedText Formatted(IBrush brush) => new(text, System.Globalization.CultureInfo.CurrentCulture, FlowDirection.LeftToRight, typeface, fontSize, brush);
        var textWidth = text.Length > 0 ? (int)Math.Ceiling(Formatted(Brushes.White).Width) : 0;
        var logoHeight = logo is null ? 0 : fontSize * LogoHeight;
        var logoWidth = logo is null ? 0 : Math.Round(logo.PixelSize.Width / (double)logo.PixelSize.Height * logoHeight);
        var gap = logo is not null && text.Length > 0 ? Math.Round(fontSize * Gap) : 0;
        var padding = Math.Ceiling(fontSize * 0.25);
        var width = (int)Math.Max(1, Math.Min(logoWidth + gap + textWidth + padding * 2, imageWidth));
        var height = (int)Math.Max(1, Math.Min(Math.Max(logoHeight, Math.Ceiling(fontSize * 1.4)) + padding * 2, imageHeight));

        using var target = new RenderTargetBitmap(new PixelSize(width, height), new Vector(96, 96));
        using (var context = target.CreateDrawingContext())
        using (context.PushOpacity(mark.Opacity))
        {
            if (logo is not null)
            {
                context.DrawImage(logo, new Rect(padding, (height - logoHeight) / 2, logoWidth, logoHeight));
            }
            if (text.Length > 0)
            {
                var white = Formatted(Brushes.White);
                var origin = new Point(padding + logoWidth + gap, (height - white.Height) / 2);
                var shadow = Formatted(new SolidColorBrush(Color.FromArgb(140, 0, 0, 0)));
                var offset = Math.Max(1, fontSize * 0.04);
                context.DrawText(shadow, origin + new Point(offset, offset));
                context.DrawText(white, origin);
            }
        }
        logo?.Dispose();

        var stride = width * 4;
        var pixels = new byte[stride * height];
        unsafe
        {
            fixed (byte* buffer = pixels) target.CopyPixels(new PixelRect(0, 0, width, height), (nint)buffer, pixels.Length, stride);
        }
        // Rendered premultiplied, usually BGRA; the engine composites straight RGBA.
        var bgra = target.Format != PixelFormat.Rgba8888;
        for (var at = 0; at < pixels.Length; at += 4)
        {
            var alpha = pixels[at + 3];
            var (first, third) = (pixels[at], pixels[at + 2]);
            if (bgra) (pixels[at], pixels[at + 2]) = (third, first);
            if (alpha is > 0 and < 255)
            {
                for (var channel = 0; channel < 3; channel++) pixels[at + channel] = (byte)Math.Min(255, pixels[at + channel] * 255 / alpha);
            }
        }

        var left = mark.Position is "topLeft" or "bottomLeft";
        var top = mark.Position is "topLeft" or "topRight";
        var centred = mark.Position == "center";
        var x = centred ? (imageWidth - width) / 2 : left ? margin : imageWidth - width - margin;
        var y = centred ? (imageHeight - height) / 2 : top ? margin : imageHeight - height - margin;
        return new OverlayRaster(pixels, x, y, width, height);
    }
}
