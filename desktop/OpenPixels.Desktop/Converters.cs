using Avalonia.Data.Converters;
using Avalonia.Media;

namespace OpenPixels.Desktop;

public static class Converters
{
    private static readonly IBrush Changed = new SolidColorBrush(Color.Parse("#f0f0f4"));
    private static readonly IBrush Resting = new SolidColorBrush(Color.Parse("#a8a8b0"));
    private static readonly IBrush Idle = new SolidColorBrush(Color.Parse("#3a3a40"));
    private static readonly IBrush ShadowClip = new SolidColorBrush(Color.Parse("#3f73d8"));
    private static readonly IBrush HighlightClip = new SolidColorBrush(Color.Parse("#e0483e"));

    /// <summary>A slider's label is brighter once it has been moved.</summary>
    public static readonly IValueConverter ChangedForeground = new FuncValueConverter<bool, IBrush>(changed => changed ? Changed : Resting);

    /// <summary>The clip toggles light up when that end of the histogram is clipped.</summary>
    public static readonly IValueConverter ShadowClipBrush = new FuncValueConverter<HistogramData?, IBrush>(data => data?.Clipped(0) == true ? ShadowClip : Idle);
    public static readonly IValueConverter HighlightClipBrush = new FuncValueConverter<HistogramData?, IBrush>(data => data?.Clipped(255) == true ? HighlightClip : Idle);
}
