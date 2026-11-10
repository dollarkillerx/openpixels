using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Media;

namespace OpenPixels.Desktop;

/// <summary>
/// The OpenPixels symbol, drawn from the same shapes as Assets/logo.svg so it is sharp at
/// any size: an open aperture and a detached pixel.
/// </summary>
public sealed class LogoMark : Control
{
    private static readonly Geometry Aperture = Geometry.Parse("M80 32H164V76H108L64 120L128 184L176 136V92H224V176A48 48 0 0 1 176 224H80A48 48 0 0 1 32 176V80A48 48 0 0 1 80 32Z");
    private static readonly IBrush Coral = new SolidColorBrush(Color.Parse("#FF7A45"));
    private static readonly IBrush Amber = new SolidColorBrush(Color.Parse("#FFB547"));
    /// <summary>The symbol spans 32..224 of the SVG's 256-unit canvas.</summary>
    private const double Start = 32, Span = 192;

    public override void Render(DrawingContext context)
    {
        var scale = Math.Min(Bounds.Width, Bounds.Height) / Span;
        var left = (Bounds.Width - Span * scale) / 2;
        var top = (Bounds.Height - Span * scale) / 2;
        using var _ = context.PushTransform(Matrix.CreateTranslation(-Start, -Start) * Matrix.CreateScale(scale, scale) * Matrix.CreateTranslation(left, top));
        context.DrawGeometry(Coral, null, Aperture);
        context.FillRectangle(Amber, new Rect(180, 32, 44, 44));
    }
}

/// <summary>256-bin counts per channel of the developed image.</summary>
public sealed record HistogramData(uint[][] Channels, int Total)
{
    /// <summary>Only this share of pixels at an extreme counts as clipping, so noise does not.</summary>
    private const double ClippedShare = 0.002;

    public static HistogramData FromRgb(byte[] rgb)
    {
        uint[][] channels = [new uint[256], new uint[256], new uint[256]];
        for (var at = 0; at + 2 < rgb.Length; at += 3)
        {
            channels[0][rgb[at]]++;
            channels[1][rgb[at + 1]]++;
            channels[2][rgb[at + 2]]++;
        }
        return new(channels, rgb.Length / 3);
    }

    public bool Clipped(int bin) => Channels.Any(channel => channel[bin] / (double)Math.Max(1, Total) > ClippedShare);

    /// <summary>A filled area for one channel, or the mean of all three for -1, scaled to the size.</summary>
    public StreamGeometry Area(int channel, double width, double height)
    {
        double Count(int bin) => channel >= 0 ? Channels[channel][bin] : (Channels[0][bin] + Channels[1][bin] + Channels[2][bin]) / 3.0;
        // Square root, so a large flat area does not flatten everything else; the clipped
        // ends are left out of the scale, or clipping would squash the rest of the graph.
        var tallest = 1.0;
        for (var bin = 1; bin < 255; bin++) tallest = Math.Max(tallest, Count(bin));
        tallest = Math.Sqrt(tallest);
        var geometry = new StreamGeometry();
        using var context = geometry.Open();
        context.BeginFigure(new Point(0, height), true);
        for (var bin = 0; bin < 256; bin++)
        {
            context.LineTo(new Point(bin / 255.0 * width, height - Math.Min(1, Math.Sqrt(Count(bin)) / tallest) * height));
        }
        context.LineTo(new Point(width, height));
        context.EndFigure(true);
        return geometry;
    }
}

public sealed class HistogramView : Control
{
    public static readonly StyledProperty<HistogramData?> DataProperty = AvaloniaProperty.Register<HistogramView, HistogramData?>(nameof(Data));

    private static readonly IBrush[] Fills =
    [
        new SolidColorBrush(Color.Parse("#a0e0483e")), new SolidColorBrush(Color.Parse("#a055b04a")), new SolidColorBrush(Color.Parse("#a03f73d8")),
    ];

    static HistogramView() => AffectsRender<HistogramView>(DataProperty);

    public HistogramData? Data
    {
        get => GetValue(DataProperty);
        set => SetValue(DataProperty, value);
    }

    public override void Render(DrawingContext context)
    {
        var (width, height) = (Bounds.Width, Bounds.Height);
        context.FillRectangle(new SolidColorBrush(Color.Parse("#0f0f11")), new Rect(0, 0, width, height));
        if (Data is not { } data) return;
        for (var channel = 0; channel < 3; channel++) context.DrawGeometry(Fills[channel], null, data.Area(channel, width, height));
    }
}

/// <summary>A point curve over the histogram. Click to add a point, drag to move, double-click or drag out to remove.</summary>
public sealed class CurveEditor : Control
{
    public static readonly StyledProperty<IReadOnlyDictionary<string, double>> EditProperty =
        AvaloniaProperty.Register<CurveEditor, IReadOnlyDictionary<string, double>>(nameof(Edit), new Dictionary<string, double>());
    public static readonly StyledProperty<string> ChannelProperty = AvaloniaProperty.Register<CurveEditor, string>(nameof(Channel), "M");
    public static readonly StyledProperty<HistogramData?> HistogramProperty = AvaloniaProperty.Register<CurveEditor, HistogramData?>(nameof(Histogram));

    private const double Inset = 8;
    private const double GrabRadius = 14;
    private const double SnapDistance = 12;
    private const double MinSpacing = 0.01;
    private const int MaxPoints = 16;
    private const double RemoveDistance = 32;
    private const int PathSteps = 120;
    private const double CornerSnap = 0.02;

    private static readonly Dictionary<string, Color> Colours = new()
    {
        ["M"] = Color.Parse("#e7e7ea"), ["R"] = Color.Parse("#e0605a"), ["G"] = Color.Parse("#62b868"), ["B"] = Color.Parse("#5a8fe0"),
    };

    private IReadOnlyDictionary<string, double> _latest = new Dictionary<string, double>();
    private int? _dragging;

    static CurveEditor() => AffectsRender<CurveEditor>(EditProperty, ChannelProperty, HistogramProperty, IsEnabledProperty);

    public CurveEditor()
    {
        Cursor = new Cursor(StandardCursorType.Cross);
    }

    /// <summary>Raised with the whole edit after the curve changes.</summary>
    public event Action<Dictionary<string, double>>? Changed;

    public IReadOnlyDictionary<string, double> Edit
    {
        get => GetValue(EditProperty);
        set => SetValue(EditProperty, value);
    }

    public string Channel
    {
        get => GetValue(ChannelProperty);
        set => SetValue(ChannelProperty, value);
    }

    public HistogramData? Histogram
    {
        get => GetValue(HistogramProperty);
        set => SetValue(HistogramProperty, value);
    }

    protected override void OnPropertyChanged(AvaloniaPropertyChangedEventArgs change)
    {
        base.OnPropertyChanged(change);
        if (change.Property == EditProperty) _latest = Edit;
    }

    private double Size => Math.Max(1, Math.Min(Bounds.Width, Bounds.Height) - 2 * Inset);

    private double Added(IReadOnlyDictionary<string, double> edit, double x) => Channel == "M" ? Curves.ParametricOffset(edit, x) : 0;

    private double ShownY(IReadOnlyDictionary<string, double> edit, double x, double y) => Math.Clamp(y + Added(edit, x), 0, 1);

    public override void Render(DrawingContext context)
    {
        var size = Size;
        var colour = Colours.GetValueOrDefault(Channel, Colours["M"]);
        using var _ = context.PushTransform(Matrix.CreateTranslation(Inset, Inset));
        context.FillRectangle(new SolidColorBrush(Color.Parse("#0f0f11")), new Rect(0, 0, size, size));
        if (Histogram is { } histogram)
        {
            var channel = Array.IndexOf(Curves.Channels, Channel) - 1;
            context.DrawGeometry(new SolidColorBrush(colour, 0.18), null, histogram.Area(channel, size, size));
        }
        var grid = new Pen(new SolidColorBrush(Color.Parse("#2c2c31")), 1);
        foreach (var at in new[] { 0.25, 0.5, 0.75 })
        {
            context.DrawLine(grid, new Point(at * size, 0), new Point(at * size, size));
            context.DrawLine(grid, new Point(0, at * size), new Point(size, at * size));
        }
        context.DrawLine(new Pen(new SolidColorBrush(Color.Parse("#3a3a40")), 1), new Point(0, size), new Point(size, 0));
        context.DrawRectangle(null, new Pen(new SolidColorBrush(Color.Parse("#3a3a40")), 1), new Rect(0, 0, size, size));

        var edit = Edit;
        var points = Curves.PointsOf(edit, Channel);
        var spline = Curves.SplineThrough(points);
        var opacity = IsEnabled ? 1 : 0.4;
        var curve = new StreamGeometry();
        using (var path = curve.Open())
        {
            for (var i = 0; i <= PathSteps; i++)
            {
                var x = i / (double)PathSteps;
                var point = new Point(x * size, (1 - ShownY(edit, x, spline(x))) * size);
                if (i == 0) path.BeginFigure(point, false);
                else path.LineTo(point);
            }
            path.EndFigure(false);
        }
        var brush = new SolidColorBrush(colour, opacity);
        context.DrawGeometry(null, new Pen(brush, 2, lineJoin: PenLineJoin.Round), curve);
        foreach (var point in points)
        {
            context.DrawEllipse(brush, null, new Point(point.X * size, (1 - ShownY(edit, point.X, point.Y)) * size), 5, 5);
        }
    }

    private (double X, double Y) PointerAt(PointerEventArgs e)
    {
        var position = e.GetPosition(this);
        return (position.X - Inset, position.Y - Inset);
    }

    private int NearestPoint(IReadOnlyDictionary<string, double> edit, IReadOnlyList<CurvePoint> list, double px, double py)
    {
        var (best, bestDistance) = (-1, GrabRadius);
        for (var index = 0; index < list.Count; index++)
        {
            var distance = Math.Sqrt(Math.Pow(list[index].X * Size - px, 2) + Math.Pow((1 - ShownY(edit, list[index].X, list[index].Y)) * Size - py, 2));
            if (distance <= bestDistance) (best, bestDistance) = (index, distance);
        }
        return best;
    }

    private void Write(IReadOnlyList<CurvePoint> points)
    {
        var updated = Curves.WithPoints(_latest, Channel, points);
        _latest = updated;
        Changed?.Invoke(updated);
    }

    protected override void OnPointerPressed(PointerPressedEventArgs e)
    {
        base.OnPointerPressed(e);
        if (!IsEnabled || !e.GetCurrentPoint(this).Properties.IsLeftButtonPressed) return;
        var list = Curves.PointsOf(_latest, Channel);
        var (px, py) = PointerAt(e);
        if (e.ClickCount == 2)
        {
            RemoveAt(list, px, py);
            return;
        }
        e.Pointer.Capture(this);
        var grabbed = NearestPoint(_latest, list, px, py);
        if (grabbed >= 0)
        {
            _dragging = grabbed;
            return;
        }
        var x = Math.Clamp(px / Size, 0, 1);
        var after = list.ToList().FindIndex(point => point.X > x);
        if (list.Count >= MaxPoints || after <= 0 || x - list[after - 1].X < MinSpacing || list[after].X - x < MinSpacing) return;
        var onCurve = Curves.SplineThrough(list)(x);
        var pressedY = 1 - py / Size - Added(_latest, x);
        var nearCurve = Math.Abs(pressedY - onCurve) * Size <= SnapDistance;
        Write([.. list.Take(after), new CurvePoint(x, nearCurve ? onCurve : Math.Clamp(pressedY, 0, 1)), .. list.Skip(after)]);
        _dragging = after;
    }

    protected override void OnPointerMoved(PointerEventArgs e)
    {
        base.OnPointerMoved(e);
        if (_dragging is not { } index || !IsEnabled) return;
        var list = Curves.PointsOf(_latest, Channel);
        if (index >= list.Count) return;
        var (px, py) = PointerAt(e);
        var last = list.Count - 1;
        var outside = px < -RemoveDistance || px > Size + RemoveDistance || py < -RemoveDistance || py > Size + RemoveDistance;
        if (outside && index > 0 && index < last)
        {
            _dragging = null;
            Write(list.Where((_, position) => position != index).ToList());
            return;
        }
        var x = Math.Clamp(px / Size, index > 0 ? list[index - 1].X + MinSpacing : 0, index < last ? list[index + 1].X - MinSpacing : 1);
        var y = Math.Clamp(1 - py / Size - Added(_latest, x), 0, 1);
        if (index == 0 || index == last)
        {
            double corner = index == 0 ? 0 : 1;
            if (Math.Sqrt(Math.Pow(x - corner, 2) + Math.Pow(y - corner, 2)) < CornerSnap) (x, y) = (corner, corner);
        }
        Write(list.Select((point, position) => position == index ? new CurvePoint(x, y) : point).ToList());
    }

    protected override void OnPointerReleased(PointerReleasedEventArgs e)
    {
        base.OnPointerReleased(e);
        _dragging = null;
        e.Pointer.Capture(null);
    }

    private void RemoveAt(IReadOnlyList<CurvePoint> list, double px, double py)
    {
        var index = NearestPoint(_latest, list, px, py);
        if (index < 0) return;
        if (index > 0 && index < list.Count - 1) Write(list.Where((_, position) => position != index).ToList());
        else Write(list.Select((point, position) => position == index ? (index == 0 ? new CurvePoint(0, 0) : new CurvePoint(1, 1)) : point).ToList());
    }
}

public sealed class WheelPickedEventArgs(double hue, double amount) : EventArgs
{
    public double Hue { get; } = hue;
    public double Amount { get; } = amount;
}

/// <summary>Picks a hue by angle and a strength by distance from the centre. Double-click resets.</summary>
public sealed class ColorWheel : Control
{
    public static readonly StyledProperty<double> HueProperty = AvaloniaProperty.Register<ColorWheel, double>(nameof(Hue), defaultBindingMode: Avalonia.Data.BindingMode.TwoWay);
    public static readonly StyledProperty<double> AmountProperty = AvaloniaProperty.Register<ColorWheel, double>(nameof(Amount), defaultBindingMode: Avalonia.Data.BindingMode.TwoWay);

    private bool _dragging;

    static ColorWheel() => AffectsRender<ColorWheel>(HueProperty, AmountProperty, IsEnabledProperty);

    /// <summary>Raised with hue and amount while the wheel is dragged or reset.</summary>
    public event EventHandler<WheelPickedEventArgs>? Picked;

    public double Hue
    {
        get => GetValue(HueProperty);
        set => SetValue(HueProperty, value);
    }

    public double Amount
    {
        get => GetValue(AmountProperty);
        set => SetValue(AmountProperty, value);
    }

    private double Radius => Math.Min(Bounds.Width, Bounds.Height) / 2;

    public override void Render(DrawingContext context)
    {
        var radius = Radius;
        var centre = new Point(Bounds.Width / 2, Bounds.Height / 2);
        var stops = new GradientStops();
        for (var step = 0; step <= 12; step++) stops.Add(new GradientStop(HsvColour(step * 30, 0.75, 0.85), step / 12.0));
        using (context.PushOpacity(IsEnabled ? 1 : 0.4))
        {
            context.DrawEllipse(new ConicGradientBrush { GradientStops = stops, Center = RelativePoint.Center }, null, centre, radius, radius);
            var fade = new RadialGradientBrush
            {
                GradientStops = [new GradientStop(Color.Parse("#ff6a6a70"), 0), new GradientStop(Color.Parse("#006a6a70"), 1)],
            };
            context.DrawEllipse(fade, null, centre, radius, radius);
            var radians = Hue * Math.PI / 180;
            var dot = new Point(centre.X + Math.Sin(radians) * Amount * radius, centre.Y - Math.Cos(radians) * Amount * radius);
            context.DrawEllipse(Brushes.White, new Pen(Brushes.Black, 1), dot, 5, 5);
        }
    }

    public static Color HsvColour(double hue, double saturation, double value)
    {
        double Channel(double n)
        {
            var k = (n + hue / 60) % 6;
            return value - value * saturation * Math.Max(0, Math.Min(Math.Min(k, 4 - k), 1));
        }
        return Color.FromRgb((byte)(Channel(5) * 255), (byte)(Channel(3) * 255), (byte)(Channel(1) * 255));
    }

    private void Update(PointerEventArgs e)
    {
        var position = e.GetPosition(this);
        var (dx, dy) = (position.X - Bounds.Width / 2, position.Y - Bounds.Height / 2);
        var angle = Math.Atan2(dx, -dy) * 180 / Math.PI;
        var (hue, amount) = ((angle + 360) % 360, Math.Min(Math.Sqrt(dx * dx + dy * dy) / Radius, 1));
        (Hue, Amount) = (hue, amount);
        Picked?.Invoke(this, new WheelPickedEventArgs(hue, amount));
    }

    protected override void OnPointerPressed(PointerPressedEventArgs e)
    {
        base.OnPointerPressed(e);
        if (!IsEnabled) return;
        if (e.ClickCount == 2)
        {
            (Hue, Amount) = (0, 0);
            Picked?.Invoke(this, new WheelPickedEventArgs(0, 0));
            return;
        }
        e.Pointer.Capture(this);
        _dragging = true;
        Update(e);
    }

    protected override void OnPointerMoved(PointerEventArgs e)
    {
        base.OnPointerMoved(e);
        if (_dragging) Update(e);
    }

    protected override void OnPointerReleased(PointerReleasedEventArgs e)
    {
        base.OnPointerReleased(e);
        _dragging = false;
        e.Pointer.Capture(null);
    }
}

/// <summary>What the retouching and local tools show on the stage. Never takes input itself.</summary>
public sealed class ToolOverlay : Control
{
    private static readonly IPen Outline = new Pen(new SolidColorBrush(Color.FromArgb(220, 255, 255, 255)), 1.5);
    private static readonly IPen Shadow = new Pen(new SolidColorBrush(Color.FromArgb(120, 0, 0, 0)), 3);
    private static readonly IBrush EraseFill = new SolidColorBrush(Color.FromArgb(110, 255, 90, 60));

    public ToolOverlay()
    {
        IsHitTestVisible = false;
    }

    /// <summary>Shapes in this control's coordinates; set by the window from the view model.</summary>
    public ToolOverlayState? State { get; set; }
    /// <summary>Device pixels per unit of this control.</summary>
    public double Scaling { get; set; } = 1;
    public Point? Pointer { get; set; }

    public override void Render(DrawingContext context)
    {
        if (State is not { } state) return;
        Point P(Point device) => new(device.X / Scaling, device.Y / Scaling);
        foreach (var (centre, radius) in state.Dabs) context.DrawEllipse(EraseFill, null, P(centre), radius / Scaling, radius / Scaling);
        foreach (var (a, b, radial, _) in state.Handles)
        {
            var (start, end) = (P(a), P(b));
            if (radial)
            {
                var radius = Math.Sqrt(Math.Pow(end.X - start.X, 2) + Math.Pow(end.Y - start.Y, 2));
                context.DrawEllipse(null, Shadow, start, radius, radius);
                context.DrawEllipse(null, Outline, start, radius, radius);
                context.DrawEllipse(null, Outline, start, radius * 0.55, radius * 0.55);
            }
            else
            {
                // Lines across the gradient's start and end, square to its direction.
                var (dx, dy) = (end.X - start.X, end.Y - start.Y);
                var length = Math.Max(1, Math.Sqrt(dx * dx + dy * dy));
                var (nx, ny) = (-dy / length * 2000, dx / length * 2000);
                foreach (var at in new[] { start, end })
                {
                    context.DrawLine(Shadow, new Point(at.X - nx, at.Y - ny), new Point(at.X + nx, at.Y + ny));
                    context.DrawLine(Outline, new Point(at.X - nx, at.Y - ny), new Point(at.X + nx, at.Y + ny));
                }
                context.DrawLine(Outline, start, end);
            }
            foreach (var at in new[] { start, end })
            {
                context.DrawEllipse(Brushes.White, Shadow, at, 5, 5);
            }
        }
        if (state.StampSource is { } source)
        {
            var at = P(source);
            var radius = Math.Max(3, state.BrushRadius / Scaling);
            context.DrawEllipse(null, Shadow, at, radius, radius);
            context.DrawEllipse(null, new Pen(new SolidColorBrush(Color.Parse("#FF7A45")), 1.5), at, radius, radius);
            context.DrawLine(Outline, new Point(at.X - 4, at.Y), new Point(at.X + 4, at.Y));
            context.DrawLine(Outline, new Point(at.X, at.Y - 4), new Point(at.X, at.Y + 4));
        }
        if (Pointer is { } pointer && state.BrushRadius > 0)
        {
            var radius = Math.Max(2, state.BrushRadius / Scaling);
            context.DrawEllipse(null, Shadow, pointer, radius, radius);
            context.DrawEllipse(null, Outline, pointer, radius, radius);
        }
    }
}

/// <summary>The crop rectangle over the whole frame, with a handle at each corner.</summary>
public sealed class CropOverlay : Control
{
    private const double HandleSize = 14;

    private (CropHandle Handle, Point Start, Crop Crop)? _drag;

    /// <summary>Where the whole frame is drawn, in this control's coordinates.</summary>
    public Box ImageBox { get; set; }
    public Crop Crop { get; set; } = Crop.Full;
    public double? Ratio { get; set; }
    public double FrameWidth { get; set; } = 1;
    public double FrameHeight { get; set; } = 1;

    public event Action<Crop>? Changed;

    private Rect CropRect => new(ImageBox.Left + Crop.X * ImageBox.Width, ImageBox.Top + Crop.Y * ImageBox.Height, Crop.W * ImageBox.Width, Crop.H * ImageBox.Height);

    public override void Render(DrawingContext context)
    {
        var rect = CropRect;
        var shade = new SolidColorBrush(Color.FromArgb(140, 0, 0, 0));
        var image = new Rect(ImageBox.Left, ImageBox.Top, ImageBox.Width, ImageBox.Height);
        context.FillRectangle(shade, new Rect(image.Left, image.Top, image.Width, Math.Max(0, rect.Top - image.Top)));
        context.FillRectangle(shade, new Rect(image.Left, rect.Bottom, image.Width, Math.Max(0, image.Bottom - rect.Bottom)));
        context.FillRectangle(shade, new Rect(image.Left, rect.Top, Math.Max(0, rect.Left - image.Left), rect.Height));
        context.FillRectangle(shade, new Rect(rect.Right, rect.Top, Math.Max(0, image.Right - rect.Right), rect.Height));
        var thirds = new Pen(new SolidColorBrush(Color.FromArgb(110, 255, 255, 255)), 1);
        for (var i = 1; i < 3; i++)
        {
            context.DrawLine(thirds, new Point(rect.Left + rect.Width * i / 3, rect.Top), new Point(rect.Left + rect.Width * i / 3, rect.Bottom));
            context.DrawLine(thirds, new Point(rect.Left, rect.Top + rect.Height * i / 3), new Point(rect.Right, rect.Top + rect.Height * i / 3));
        }
        context.DrawRectangle(null, new Pen(Brushes.White, 1.5), rect);
        foreach (var corner in Corners(rect)) context.FillRectangle(Brushes.White, new Rect(corner.Point.X - HandleSize / 2, corner.Point.Y - HandleSize / 2, HandleSize, HandleSize));
    }

    private static (CropHandle Handle, Point Point)[] Corners(Rect rect) =>
    [
        (CropHandle.NorthWest, rect.TopLeft), (CropHandle.NorthEast, rect.TopRight), (CropHandle.SouthWest, rect.BottomLeft), (CropHandle.SouthEast, rect.BottomRight),
    ];

    protected override void OnPointerPressed(PointerPressedEventArgs e)
    {
        base.OnPointerPressed(e);
        var position = e.GetPosition(this);
        var rect = CropRect;
        CropHandle? handle = null;
        foreach (var corner in Corners(rect))
        {
            if (Math.Abs(position.X - corner.Point.X) <= HandleSize && Math.Abs(position.Y - corner.Point.Y) <= HandleSize) handle = corner.Handle;
        }
        if (handle is null && rect.Contains(position)) handle = CropHandle.Move;
        if (handle is null) return;
        e.Pointer.Capture(this);
        e.Handled = true;
        _drag = (handle.Value, position, Crop);
    }

    protected override void OnPointerMoved(PointerEventArgs e)
    {
        base.OnPointerMoved(e);
        if (_drag is not { } drag || ImageBox.Width <= 0 || ImageBox.Height <= 0) return;
        var position = e.GetPosition(this);
        var next = Frame.DragCrop(drag.Crop, drag.Handle, (position.X - drag.Start.X) / ImageBox.Width, (position.Y - drag.Start.Y) / ImageBox.Height, Ratio, FrameWidth, FrameHeight);
        Changed?.Invoke(next);
    }

    protected override void OnPointerReleased(PointerReleasedEventArgs e)
    {
        base.OnPointerReleased(e);
        _drag = null;
        e.Pointer.Capture(null);
    }
}
