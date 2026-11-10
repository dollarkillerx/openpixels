namespace OpenPixels.Desktop;

public readonly record struct Crop(double X, double Y, double W, double H)
{
    public static readonly Crop Full = new(0, 0, 1, 1);
}

public readonly record struct Box(double Left, double Top, double Width, double Height);

/// <summary>How the framed image is shown: zoom times its fitted size, around (X, Y) in 0..=1.</summary>
public readonly record struct View(double Zoom, double X, double Y)
{
    public static readonly View Fit = new(1, 0.5, 0.5);
}

public enum CropHandle { Move, NorthWest, NorthEast, SouthWest, SouthEast }

/// <summary>Crop and view geometry, as in web/src/frame.ts. Mirrors op-core's Geometry and op-gpu's VisibleArea.</summary>
public static class Frame
{
    private const double MinCrop = 0.05;

    private static double Get(IReadOnlyDictionary<string, double> edit, string key, double fallback) => edit.TryGetValue(key, out var value) ? value : fallback;

    public static Crop CropOf(IReadOnlyDictionary<string, double> edit) =>
        new(Get(edit, "cropX", 0), Get(edit, "cropY", 0), Get(edit, "cropW", 1), Get(edit, "cropH", 1));

    public static void SetCrop(Dictionary<string, double> edit, Crop crop)
    {
        void Set(string key, double value, double resting)
        {
            if (value == resting) edit.Remove(key);
            else edit[key] = value;
        }
        Set("cropX", crop.X, 0);
        Set("cropY", crop.Y, 0);
        Set("cropW", crop.W, 1);
        Set("cropH", crop.H, 1);
    }

    /// <summary>Size of the image after its quarter turns.</summary>
    public static (double Width, double Height) FrameSize(double width, double height, int quarter) =>
        ((quarter % 4) + 4) % 4 is 1 or 3 ? (height, width) : (width, height);

    /// <summary>The largest box of the given aspect ratio centred in a stage.</summary>
    public static Box FitBox(double aspect, double stageWidth, double stageHeight)
    {
        var width = Math.Min(stageWidth, stageHeight * aspect);
        var height = width / aspect;
        return new((stageWidth - width) / 2, (stageHeight - height) / 2, width, height);
    }

    /// <summary>Where a zoomed image sits on the stage.</summary>
    public static Box ViewBox(Box fit, double stageWidth, double stageHeight, View view)
    {
        (double Size, double Start) Place(double fitted, double stage, double centre)
        {
            var size = fitted * view.Zoom;
            if (size <= stage) return (size, (stage - size) / 2);
            var span = stage / size;
            return (size, -Math.Min(1 - span, Math.Max(0, centre - span / 2)) * size);
        }
        var x = Place(fit.Width, stageWidth, view.X);
        var y = Place(fit.Height, stageHeight, view.Y);
        return new(x.Start, y.Start, x.Size, y.Size);
    }

    /// <summary>The largest centred crop with ratio (width / height, in pixels).</summary>
    public static Crop CropForRatio(double ratio, double frameWidth, double frameHeight)
    {
        var frameAspect = frameWidth / frameHeight;
        var (w, h) = ratio > frameAspect ? (1.0, frameAspect / ratio) : (ratio / frameAspect, 1.0);
        return new((1 - w) / 2, (1 - h) / 2, w, h);
    }

    private static bool InsideRotatedImage(Crop crop, double degrees, double width, double height)
    {
        var radians = degrees * Math.PI / 180;
        var (sin, cos) = (Math.Sin(radians), Math.Cos(radians));
        foreach (var (s, t) in new[] { (0, 0), (1, 0), (0, 1), (1, 1) })
        {
            var x = (crop.X + s * crop.W - 0.5) * width;
            var y = (crop.Y + t * crop.H - 0.5) * height;
            if (Math.Abs(x * cos + y * sin) > width / 2 + 1e-3 || Math.Abs(-x * sin + y * cos) > height / 2 + 1e-3) return false;
        }
        return true;
    }

    /// <summary>Shrinks a crop about its centre until it holds no area from outside the image.</summary>
    public static Crop ShrinkToImage(Crop crop, double degrees, double frameWidth, double frameHeight)
    {
        var current = crop;
        // Moving towards the centre is what frees the corners when the crop is off-centre.
        for (var step = 0; step < 200 && !InsideRotatedImage(current, degrees, frameWidth, frameHeight); step++)
        {
            var (w, h) = (current.W * 0.99, current.H * 0.99);
            var (centreX, centreY) = (current.X + current.W / 2, current.Y + current.H / 2);
            const double towardsMiddle = 0.98;
            current = new(0.5 + (centreX - 0.5) * towardsMiddle - w / 2, 0.5 + (centreY - 0.5) * towardsMiddle - h / 2, w, h);
        }
        return current;
    }

    /// <summary>Source UV shown at output position (s, t). Mirrors Geometry::uv_map in op-core; ignores lens correction.</summary>
    public static UvPoint OutputToSource(double s, double t, IReadOnlyDictionary<string, double> edit, double width, double height)
    {
        var quarter = (int)Get(edit, "quarter", 0);
        var (frameWidth, frameHeight) = FrameSize(width, height, quarter);
        var crop = CropOf(edit);
        var radians = Get(edit, "rotate", 0) * Math.PI / 180;
        var (sin, cos) = (Math.Sin(radians), Math.Cos(radians));
        var x = (crop.X + s * crop.W - 0.5) * frameWidth;
        var y = (crop.Y + t * crop.H - 0.5) * frameHeight;
        var (ux, uy) = (x * cos + y * sin, -x * sin + y * cos);
        var (sx, sy) = (((quarter % 4) + 4) % 4) switch
        {
            1 => (uy, -ux),
            2 => (-ux, -uy),
            3 => (-uy, ux),
            _ => (ux, uy),
        };
        return new(sx / width + 0.5, sy / height + 0.5);
    }

    /// <summary>Output position (s, t) showing source UV (u, v): the inverse of OutputToSource.</summary>
    public static UvPoint SourceToOutput(double u, double v, IReadOnlyDictionary<string, double> edit, double width, double height)
    {
        var origin = OutputToSource(0, 0, edit, width, height);
        var endS = OutputToSource(1, 0, edit, width, height);
        var endT = OutputToSource(0, 1, edit, width, height);
        var (ax, ay, bx, by) = (endS.X - origin.X, endS.Y - origin.Y, endT.X - origin.X, endT.Y - origin.Y);
        var determinant = ax * by - ay * bx;
        var (du, dv) = (u - origin.X, v - origin.Y);
        return new((du * by - dv * bx) / determinant, (dv * ax - du * ay) / determinant);
    }

    /// <summary>A crop after dragging a handle by a fraction of the frame. ratio locks the shape.</summary>
    public static Crop DragCrop(Crop start, CropHandle handle, double dx, double dy, double? ratio, double frameWidth, double frameHeight)
    {
        if (handle == CropHandle.Move)
        {
            return start with { X = Math.Min(Math.Max(start.X + dx, 0), 1 - start.W), Y = Math.Min(Math.Max(start.Y + dy, 0), 1 - start.H) };
        }
        var fromLeft = handle is CropHandle.NorthWest or CropHandle.SouthWest;
        var fromTop = handle is CropHandle.NorthWest or CropHandle.NorthEast;
        var (right, bottom) = (start.X + start.W, start.Y + start.H);
        var w = Math.Max(MinCrop, fromLeft ? start.W - dx : start.W + dx);
        var h = Math.Max(MinCrop, fromTop ? start.H - dy : start.H + dy);
        w = Math.Min(w, fromLeft ? right : 1 - start.X);
        h = Math.Min(h, fromTop ? bottom : 1 - start.Y);
        if (ratio is { } locked)
        {
            var heightForWidth = w * frameWidth / locked / frameHeight;
            if (heightForWidth <= h) h = heightForWidth;
            else w = h * frameHeight * locked / frameWidth;
            var maxH = fromTop ? bottom : 1 - start.Y;
            var maxW = fromLeft ? right : 1 - start.X;
            var fit = Math.Min(1, Math.Min(maxW / w, maxH / h));
            w *= fit;
            h *= fit;
        }
        return new(fromLeft ? right - w : start.X, fromTop ? bottom - h : start.Y, w, h);
    }
}
