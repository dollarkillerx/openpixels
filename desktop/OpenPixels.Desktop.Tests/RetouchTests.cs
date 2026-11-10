using Avalonia.Headless.XUnit;
using Avalonia.Media.Imaging;
using Xunit;
using static OpenPixels.Desktop.Tests.MainWindowTests;

namespace OpenPixels.Desktop.Tests;

public sealed class LocalMaskTests
{
    private static byte At(byte[] rgba, int width, int x, int y, int channel = 0) => rgba[(y * width + x) * 4 + channel];

    [Fact]
    public void A_linear_gradient_runs_from_full_to_none()
    {
        var layer = new LocalLayer { Kind = LocalKind.Linear, A = new(0.25, 0.5), B = new(0.75, 0.5) };
        var mask = LocalMasks.Render([layer], 100, 10, new Dictionary<string, ModelMask>());
        Assert.Equal(255, At(mask, 100, 10, 5));
        Assert.InRange(At(mask, 100, 50, 5), 120, 135);
        Assert.Equal(0, At(mask, 100, 90, 5));
    }

    [Fact]
    public void A_radial_is_solid_inside_and_soft_at_the_edge_and_layers_fill_channels()
    {
        var radial = new LocalLayer { Kind = LocalKind.Radial, A = new(0.5, 0.5), B = new(0.9, 0.5) };
        var full = new LocalLayer { Kind = LocalKind.Full, Invert = true };
        var mask = LocalMasks.Render([radial, full], 100, 100, new Dictionary<string, ModelMask>());
        Assert.Equal(255, At(mask, 100, 50, 50));
        Assert.InRange(At(mask, 100, 80, 50), 60, 200);
        Assert.Equal(0, At(mask, 100, 2, 2));
        // The inverted full layer is in the second channel and covers nothing.
        Assert.Equal(0, At(mask, 100, 50, 50, 1));
    }

    [Fact]
    public void Brushing_adds_and_erasing_takes_away()
    {
        var layer = new LocalLayer { Kind = LocalKind.Brush, Radius = 0.1, Strokes = [[new(0.2, 0.5), new(0.8, 0.5)]], Erase = [[new(0.8, 0.5)]] };
        var mask = LocalMasks.Render([layer], 100, 100, new Dictionary<string, ModelMask>());
        Assert.True(At(mask, 100, 30, 50) > 150);
        Assert.True(At(mask, 100, 80, 50) < At(mask, 100, 30, 50) / 2);
        Assert.Equal(0, At(mask, 100, 50, 5));
    }

    [Fact]
    public void A_background_layer_is_the_inverse_of_the_subject()
    {
        var subject = new ModelMask([0, 255, 255, 0], 2, 2);
        var mask = LocalMasks.Render([new LocalLayer { Kind = LocalKind.Background }], 2, 2, new Dictionary<string, ModelMask> { ["subject"] = subject });
        Assert.Equal([255, 0, 0, 255], Enumerable.Range(0, 4).Select(i => mask[i * 4]).ToArray());
    }

    [Fact]
    public void An_erase_crop_is_at_least_the_model_size_and_inside_the_image()
    {
        var crop = Erasing.Crop([new EraseDab(0.98, 0.5, 0.01)], 3000, 2000);
        Assert.Equal(512, crop.Width);
        Assert.Equal(3000, crop.X + crop.Width);
        var (mask, alpha) = Erasing.Rasterize([new EraseDab(0.98, 0.5, 0.01)], crop, 3000, 2000);
        Assert.Contains(1f, mask);
        Assert.Contains((byte)255, alpha);
    }

    [Fact]
    public void Stray_sky_below_the_horizon_is_removed()
    {
        // Sky along the top row, and a separate bright patch lower down.
        var mask = new byte[10 * 10];
        for (var x = 0; x < 10; x++) mask[x] = 255;
        mask[8 * 10 + 5] = 255;
        AiModels.RemoveStraySky(mask, 10, 10);
        Assert.Equal(255, mask[3]);
        Assert.Equal(0, mask[8 * 10 + 5]);
    }
}

public sealed class RetouchTests : IDisposable
{
    private readonly string _folder = Directory.CreateTempSubdirectory("openpixels-retouch").FullName;

    public RetouchTests() => Environment.SetEnvironmentVariable("OPENPIXELS_DATA_DIR", Path.Combine(_folder, "data"));

    public void Dispose() => Directory.Delete(_folder, recursive: true);

    /// <summary>A mid-grey photo with a dark blot in the middle.</summary>
    private string Blotted() => MakePhoto(_folder, "blot.png", 400, 300, pixel: (x, y) =>
        Math.Pow(x - 200, 2) + Math.Pow(y - 150, 2) < 64 ? ((byte)20, (byte)20, (byte)20) : ((byte)(150 + (x % 7)), (byte)(140 + (y % 5)), (byte)120));

    private static async Task<(MainWindow Window, MainViewModel Model)> Open(params string[] paths)
    {
        var window = new MainWindow { Width = 1400, Height = 880 };
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos(paths);
        await WaitFor(() => model.CanEdit && model.Preview is not null, "the preview", model);
        return (window, model);
    }

    /// <summary>The stage position, in device pixels, showing a point of the image.</summary>
    private static (double X, double Y) StageAt(MainViewModel model, double u, double v)
    {
        var at = model.StageOf(new UvPoint(u, v));
        return (at.X, at.Y);
    }

    private static (int R, int G, int B) PixelAt(MainViewModel model, double u, double v)
    {
        var (x, y) = StageAt(model, u, v);
        var frame = (WriteableBitmap)model.Preview!;
        using var locked = frame.Lock();
        var bytes = new byte[4];
        System.Runtime.InteropServices.Marshal.Copy(locked.Address + (int)y * locked.RowBytes + (int)x * 4, bytes, 0, 4);
        return (bytes[0], bytes[1], bytes[2]);
    }

    [AvaloniaFact]
    public async Task Spot_healing_removes_a_blot_and_is_kept()
    {
        var path = Blotted();
        var (window, model) = await Open(path);
        Assert.True(PixelAt(model, 0.5, 0.5).G < 60);
        model.HealRadius = 0.03;
        model.ChooseToolCommand.Execute("Spot");
        var (x, y) = StageAt(model, 0.5, 0.5);
        await model.ToolPressAsync(x, y, alt: false);
        model.ToolRelease();
        Assert.True(model.HasHeals, model.ToolHint);
        await WaitFor(() => PixelAt(model, 0.5, 0.5).G > 100, "the healed blot", model);
        window.Close();

        var (_, again) = await Open(path);
        Assert.True(again.HasHeals);
        await WaitFor(() => PixelAt(again, 0.5, 0.5).G > 100, "the healed blot after reopening", again);
        await again.UndoHealCommand.ExecuteAsync(null);
        await WaitFor(() => PixelAt(again, 0.5, 0.5).G < 60, "the blot back", again);
    }

    [AvaloniaFact]
    public async Task A_linear_gradient_brightens_one_side_and_undo_removes_it()
    {
        var (_, model) = await Open(MakePhoto(_folder, "grey.png", 400, 300, solid: (100, 100, 100)));
        var before = PixelAt(model, 0.1, 0.5).G;
        model.ChooseToolCommand.Execute("Linear");
        var (ax, ay) = StageAt(model, 0.3, 0.5);
        var (bx, by) = StageAt(model, 0.7, 0.5);
        await model.ToolPressAsync(ax, ay, alt: false);
        await model.ToolMoveAsync(bx, by);
        model.ToolRelease();
        Assert.Single(model.Layers);
        Assert.Equal(RetouchTool.None, model.Tool);
        Assert.Equal("添加线性渐变", model.HistoryRows[^1].Label);
        await WaitFor(() => PixelAt(model, 0.1, 0.5).G > before + 15, "the brightened left side", model);
        Assert.True(Math.Abs(PixelAt(model, 0.9, 0.5).G - before) < 4, "the right side is untouched");

        model.Undo();
        Assert.Empty(model.Layers);
        await WaitFor(() => Math.Abs(PixelAt(model, 0.1, 0.5).G - before) < 4, "the gradient gone", model);
        model.Redo();
        Assert.Single(model.Layers);
    }

    [AvaloniaFact]
    public async Task A_brush_layer_adjusts_only_where_painted()
    {
        var (_, model) = await Open(MakePhoto(_folder, "grey.png", 400, 300, solid: (100, 100, 100)));
        var before = PixelAt(model, 0.5, 0.5).G;
        model.ChooseToolCommand.Execute("Brush");
        model.LocalBrushRadius = 0.05;
        var (x, y) = StageAt(model, 0.5, 0.5);
        await model.ToolPressAsync(x, y, alt: false);
        await model.ToolMoveAsync(x + 20, y);
        model.ToolRelease();
        await WaitFor(() => PixelAt(model, 0.5, 0.5).G > before + 10, "the painted area brighter", model);
        Assert.True(Math.Abs(PixelAt(model, 0.1, 0.1).G - before) < 4);
        model.Slider("local0Exposure").Value = -1;
        await WaitFor(() => PixelAt(model, 0.5, 0.5).G < before - 10, "the painted area darker", model);
    }

    [AvaloniaFact]
    public async Task AI_finds_the_subject_and_erases_a_blot()
    {
        if (!AiModels.Has(AiModels.Matte.File) || !AiModels.Has(AiModels.Inpaint.File)) return;
        var (_, model) = await Open(Blotted());
        await model.AddAiLayerCommand.ExecuteAsync("Subject");
        Assert.Single(model.Layers);
        Assert.StartsWith("已识别", model.RetouchStatus);

        model.ChooseToolCommand.Execute("Erase");
        model.HealRadius = 0.03;
        var (x, y) = StageAt(model, 0.5, 0.5);
        await model.ToolPressAsync(x, y, alt: false);
        model.ToolRelease();
        Assert.True(model.CanRunErase);
        await model.RunEraseCommand.ExecuteAsync(null);
        Assert.True(model.HasErases, model.RetouchStatus);
        // The subject layer brightens by default; turn it off to judge the fill itself.
        model.Slider("local0Exposure").Value = 0;
        await WaitFor(() => PixelAt(model, 0.5, 0.5).G > 100, "the blot filled in", model);
    }
}
