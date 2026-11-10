using Avalonia.Headless.XUnit;
using Avalonia.Media.Imaging;
using Xunit;
using static OpenPixels.Desktop.Tests.MainWindowTests;

namespace OpenPixels.Desktop.Tests;

public sealed class FullLogicTests
{
    [Fact]
    public void Matching_a_photo_to_itself_changes_little()
    {
        var rgb = new byte[64 * 64 * 3];
        for (var i = 0; i < rgb.Length; i += 3) (rgb[i], rgb[i + 1], rgb[i + 2]) = ((byte)(i / 3 % 64 * 4), (byte)(i / 3 / 64 * 4), 120);
        var lut = ColourMatch.Match(rgb, 3, rgb, 3);
        Assert.Equal(33, lut.Size);
        // The middle of the table maps mid grey to about mid grey.
        var centre = ((16 * 33 + 16) * 33 + 16) * 3;
        for (var channel = 0; channel < 3; channel++) Assert.InRange(lut.Data[centre + channel], 0.42, 0.58);
    }

    /// <summary>Reference values computed by web/src/colormatch.ts on the same inputs.</summary>
    [Fact]
    public void Colour_match_agrees_with_the_web_edition()
    {
        static byte[] Make(Func<int, (int R, int G, int B)> pixel)
        {
            var rgb = new byte[4096 * 3];
            for (var i = 0; i < 4096; i++) (rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]) = ((byte)pixel(i).R, (byte)pixel(i).G, (byte)pixel(i).B);
            return rgb;
        }
        var source = Make(i => (i * 7 % 200 + 20, i * 13 % 180 + 30, i * 29 % 160 + 40));
        var reference = Make(i => (i * 11 % 230 + 10, i * 3 % 120 + 60, i * 17 % 90 + 20));
        var lut = ColourMatch.Match(source, 3, reference, 3);
        int[] indices = [0, 1000, 20000, 54000, 70000, 107810];
        double[] expected = [0.06215, 0.35314, 0.11578, 0.4928, 0.45401, 0.77003];
        for (var i = 0; i < indices.Length; i++) Assert.True(Math.Abs(lut.Data[indices[i]] - expected[i]) < 2e-4, $"[{indices[i]}] {lut.Data[indices[i]]} vs {expected[i]}");
    }

    [Fact]
    public void A_warm_reference_warms_the_look()
    {
        var grey = Enumerable.Repeat((byte)128, 32 * 32 * 3).ToArray();
        var warm = new byte[32 * 32 * 3];
        for (var i = 0; i < warm.Length; i += 3) (warm[i], warm[i + 1], warm[i + 2]) = (200, 130, 70);
        // Grey with some spread, so the measured statistics are not degenerate.
        for (var i = 0; i < grey.Length; i += 3) grey[i] = grey[i + 1] = grey[i + 2] = (byte)(100 + i / 3 % 50);
        var lut = ColourMatch.Match(grey, 3, warm, 3);
        var centre = ((16 * 33 + 16) * 33 + 16) * 3;
        Assert.True(lut.Data[centre] > lut.Data[centre + 2] + 0.1, $"{lut.Data[centre]} vs {lut.Data[centre + 2]}");
    }

    [Fact]
    public void Built_in_skies_are_painted_light_towards_the_horizon()
    {
        var sky = Replacements.BuiltIn(11)!;
        Assert.Equal(1600, sky.Width);
        Assert.Equal(1600 * 1067 * 4, sky.Rgba.Length);
        int Brightness(int y) => sky.Rgba[y * 1600 * 4 + 4] + sky.Rgba[y * 1600 * 4 + 5] + sky.Rgba[y * 1600 * 4 + 6];
        Assert.True(Brightness(1060) > Brightness(5));
        Assert.Null(Replacements.BuiltIn(999));
    }
}

public sealed class FullTests : IDisposable
{
    private readonly string _folder = Directory.CreateTempSubdirectory("openpixels-full").FullName;

    public FullTests() => Environment.SetEnvironmentVariable("OPENPIXELS_DATA_DIR", Path.Combine(_folder, "data"));

    public void Dispose() => Directory.Delete(_folder, recursive: true);

    private static string Root => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../.."));

    private static async Task<(MainWindow Window, MainViewModel Model)> Open(params string[] paths)
    {
        var window = new MainWindow { Width = 1400, Height = 880 };
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos(paths);
        await WaitFor(() => model.CanEdit && model.Preview is not null, "the preview", model);
        return (window, model);
    }

    private static (int R, int G, int B) PixelAt(MainViewModel model, double u, double v)
    {
        var at = model.StageOf(new UvPoint(u, v));
        var frame = (WriteableBitmap)model.Preview!;
        using var locked = frame.Lock();
        var bytes = new byte[4];
        System.Runtime.InteropServices.Marshal.Copy(locked.Address + (int)at.Y * locked.RowBytes + (int)at.X * 4, bytes, 0, 4);
        return (bytes[0], bytes[1], bytes[2]);
    }

    [AvaloniaFact]
    public async Task Liquify_moves_pixels_undoes_and_is_kept()
    {
        // Dark on the left half, light on the right: pushing right drags dark pixels across.
        var path = MakePhoto(_folder, "halves.png", 400, 300, pixel: (x, _) => x < 200 ? ((byte)30, (byte)30, (byte)30) : ((byte)220, (byte)220, (byte)220));
        var (window, model) = await Open(path);
        Assert.True(PixelAt(model, 0.53, 0.5).G > 150);
        model.ChooseToolCommand.Execute("Liquify");
        model.LiquifyRadius = 0.15;
        model.LiquifyStrength = 1;
        var start = model.StageOf(new UvPoint(0.45, 0.5));
        await model.ToolPressAsync(start.X, start.Y, false);
        for (var step = 1; step <= 10; step++) await model.ToolMoveAsync(start.X + step * 8, start.Y);
        model.ToolRelease();
        Assert.True(model.HasWarps);
        await WaitFor(() => PixelAt(model, 0.53, 0.5).G < 120, "dark pixels pushed right", model);
        window.Close();

        var (_, again) = await Open(path);
        Assert.True(again.HasWarps);
        await WaitFor(() => PixelAt(again, 0.53, 0.5).G < 120, "the warp after reopening", again);
        await again.UndoLiquifyCommand.ExecuteAsync(null);
        await WaitFor(() => PixelAt(again, 0.53, 0.5).G > 150, "the warp undone", again);
    }

    [AvaloniaFact]
    public async Task Colour_match_takes_on_the_reference_look()
    {
        var grey = MakePhoto(_folder, "grey.png", 300, 200, pixel: (x, y) => ((byte)(90 + x % 60), (byte)(90 + x % 60), (byte)(90 + x % 60)));
        var warm = MakePhoto(_folder, "warm.png", 300, 200, pixel: (x, y) => ((byte)(170 + y % 40), (byte)(110 + y % 30), (byte)(60 + y % 20)));
        var (_, model) = await Open(grey, warm);
        model.SelectedPhoto = model.Photos[0];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto?.Name == "grey.png", "the grey photo", model);
        model.MatchReference = model.Photos[1];
        await model.MatchFromPhotoCommand.ExecuteAsync(null);
        Assert.StartsWith("追色 warm", model.SelectedLook?.Name);
        await WaitFor(() => PixelAt(model, 0.5, 0.5) is var (r, _, b) && r > b + 25, "a warmer preview", model);
    }

    /// <summary>Uses the git-ignored portrait fixture and the subject model when both are present.</summary>
    [AvaloniaFact]
    public async Task A_white_background_replaces_the_one_behind_the_subject()
    {
        var portrait = Path.Combine(Root, "web/public/fixtures/people/05-woman.jpg");
        if (!File.Exists(portrait) || !AiModels.Has(AiModels.Matte.File)) return;
        var (_, model) = await Open(portrait);
        var face = PixelAt(model, 0.5, 0.3);
        model.SelectedBackground = model.BackgroundChoices.Single(choice => choice.Id == 1);
        Assert.True(model.HasBackground);
        await WaitFor(() => PixelAt(model, 0.08, 0.15) is var (r, g, b) && r > 230 && g > 230 && b > 230, "a white corner", model);
        var after = PixelAt(model, 0.5, 0.3);
        Assert.True(Math.Abs(after.G - face.G) < 25, $"the face stays: {face} vs {after}");
        model.Undo();
        Assert.False(model.HasBackground);
    }

    /// <summary>Uses the git-ignored acne fixture and the face models when both are present.</summary>
    [AvaloniaFact]
    public async Task Skin_smoothing_finds_the_face_and_changes_only_skin()
    {
        var portrait = Path.Combine(Root, "web/public/fixtures/people/01-acne.jpg");
        if (!File.Exists(portrait) || !FaceFinder.Available) return;
        var (_, model) = await Open(portrait);
        var points = Enumerable.Range(0, 64).Select(i => (U: 0.3 + i % 8 * 0.04, V: 0.15 + i / 8 * 0.04)).ToList();
        var before = points.Select(p => PixelAt(model, p.U, p.V)).ToList();
        var corner = PixelAt(model, 0.97, 0.9);
        foreach (var slider in model.SkinGroup.Sliders) slider.Value = 1;
        await WaitFor(() => model.SkinStatus.StartsWith("检测到 1 张人脸"), "one face", model);
        double Change() => points.Select((p, i) => PixelAt(model, p.U, p.V) is var (r, g, b) ? Math.Abs(r - before[i].R) + Math.Abs(g - before[i].G) + Math.Abs(b - before[i].B) : 0).Average();
        await WaitFor(() => Change() > 2, "smoother skin", model);
        Assert.Equal(corner, PixelAt(model, 0.97, 0.9));
        model.Undo();
        model.Undo();
        await WaitFor(() => model.SkinStatus == "", "no skin status", model);
    }

    [AvaloniaFact]
    public async Task A_new_sky_replaces_the_old_one()
    {
        var lake = Path.Combine(Root, "web/public/fixtures/lite/lake.jpg");
        if (!File.Exists(lake) || !AiModels.Has(AiModels.Sky.File)) return;
        var (_, model) = await Open(lake);
        var before = PixelAt(model, 0.2, 0.05);
        model.SelectedSky = model.SkyChoices.Single(choice => choice.Id == 12);
        Assert.Equal(0.3, model.CurrentEdit["skyRelight"]);
        await WaitFor(() => PixelAt(model, 0.2, 0.05) is var (r, _, b) && Math.Abs(r - before.R) + Math.Abs(b - before.B) > 40, "a different sky", model);
    }
}
