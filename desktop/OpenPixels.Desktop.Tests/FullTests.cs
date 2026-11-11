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
    public async Task Film_simulation_pairs_any_print_and_adds_grain()
    {
        var path = MakePhoto(_folder, "flat.png", 600, 400, solid: (150, 120, 100));
        var (_, model) = await Open(path);
        var plain = PixelAt(model, 0.5, 0.5);
        Assert.False(model.FilmActive);

        model.SelectedLook = model.LookChoices.Single(look => look.Id == 46); // Vision3 500T on 2383
        Assert.True(model.FilmActive && model.FilmPrinted);
        Assert.Equal(0, model.SelectedPrint!.Id);
        await WaitFor(() => PixelAt(model, 0.5, 0.5) != plain, "the film look", model);

        model.SelectedPrint = model.PrintChoices.Single(print => print.Name == "Kodak Polymax Fine-Art Grade 3");
        Assert.Equal(12, (int)model.CurrentEdit["filmPrint"]);
        await WaitFor(() => PixelAt(model, 0.5, 0.5) is var (r, g, b) && Math.Abs(r - b) <= 2 && Math.Abs(r - g) <= 2, "a black and white print", model);

        model.Slider("filmLightGreen").Value = 0.5;
        var lit = PixelAt(model, 0.5, 0.5);
        await WaitFor(() => PixelAt(model, 0.5, 0.5) != lit, "the printer light", model);

        model.SelectedFormat = 0;
        Assert.Equal(12.5, model.CurrentEdit["filmFormat"]);
        model.Slider("filmGrain").Value = 2;
        int Distinct() => Enumerable.Range(0, 40).Select(i => PixelAt(model, 0.3 + i * 0.01, 0.5).G).Distinct().Count();
        await WaitFor(() => Distinct() >= 4, "grain on a flat photo", model);

        // A slide is projected: no print or printer lights.
        model.SelectedLook = model.LookChoices.Single(look => look.Id == 50);
        Assert.True(model.FilmActive);
        Assert.False(model.FilmPrinted);
        Assert.False(model.Slider("filmLightRed").IsVisible);
        model.SelectedLook = model.LookChoices[0];
        Assert.False(model.FilmActive);
        Assert.True(model.Slider("filmLightRed").IsVisible);
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

    /// <summary>Uses the git-ignored outdoor portrait and SAM 2.1 when both are present.</summary>
    [AvaloniaFact]
    public async Task An_object_layer_follows_a_box_and_clicks_and_undoes()
    {
        var photo = Path.Combine(Root, "web/public/fixtures/people/06-outdoor.jpg");
        if (!File.Exists(photo) || !AiModels.Segment.Available) return;
        var (_, model) = await Open(photo);
        (double X, double Y) Stage(double u, double v) => model.StageOf(new UvPoint(u, v)) is var at ? (at.X, at.Y) : default;
        var jacket = PixelAt(model, 0.25, 0.85);
        var field = PixelAt(model, 0.9, 0.55);

        await model.AddObjectLayerCommand.ExecuteAsync(null);
        Assert.True(model.IsObject);
        Assert.True(model.ActiveIsObject);
        Assert.StartsWith("已分析画面", model.RetouchStatus);

        // A box around the woman selects all of her, and only her.
        var (x0, y0) = Stage(0.25, 0.1);
        var (x1, y1) = Stage(0.75, 0.98);
        await model.ToolPressAsync(x0, y0, alt: false);
        await model.ToolMoveAsync((x0 + x1) / 2, (y0 + y1) / 2);
        await model.ToolMoveAsync(x1, y1);
        model.ToolRelease();
        Assert.True(model.ActiveHasPrompts);
        await WaitFor(() => PixelAt(model, 0.25, 0.85).G > jacket.G + 5, "a brighter jacket", model);
        Assert.True(Math.Abs(PixelAt(model, 0.9, 0.55).G - field.G) < 3, "the field stays");

        // An Option-click is kept as a point to leave out, and undo takes it away again.
        var (jx, jy) = Stage(0.25, 0.85);
        await model.ToolPressAsync(jx, jy, alt: true);
        model.ToolRelease();
        Assert.Equal(2, model.LocalLayers[0].Points.Count + (model.LocalLayers[0].Box is null ? 0 : 1));
        Assert.False(model.LocalLayers[0].Points[0].Include);
        model.Undo();
        Assert.Empty(model.LocalLayers[0].Points);

        model.ClearObjectPromptsCommand.Execute(null);
        Assert.False(model.ActiveHasPrompts);
        await WaitFor(() => Math.Abs(PixelAt(model, 0.25, 0.85).G - jacket.G) < 3, "no mask once cleared", model);

        // A single click on the jacket selects it again.
        await model.ToolPressAsync(jx, jy, alt: false);
        model.ToolRelease();
        await WaitFor(() => PixelAt(model, 0.25, 0.85).G > jacket.G + 5, "the clicked jacket brighter", model);
        Assert.True(Math.Abs(PixelAt(model, 0.9, 0.55).G - field.G) < 3, "the field stays");
    }

    /// <summary>Uses a git-ignored RAW fixture when it is present.</summary>
    [AvaloniaFact]
    public async Task Display_transforms_keep_the_exposure_and_change_the_highlights()
    {
        var raw = Path.Combine(Root, "web/public/fixtures/real/DSC00395.ARW");
        if (!File.Exists(raw)) return;
        var (_, model) = await Open(raw);
        Assert.True(model.DisplayTransformAvailable);
        var frames = new List<byte[]>();
        for (var index = 0; index < model.DisplayTransforms.Length; index++)
        {
            var before = frames.Count == 0 ? null : frames[^1];
            model.DisplayTransformIndex = index;
            Assert.Equal(index, (int)model.CurrentEdit.GetValueOrDefault("displayTransform"));
            if (before is not null) await WaitFor(() => !Pixels(model).SequenceEqual(before), $"transform {index} drawn", model);
            await Task.Delay(500);
            frames.Add(Pixels(model));
        }
        // The overall brightness stays close; the renderings differ.
        static double Luma(byte[] bgra) => Enumerable.Range(0, bgra.Length / 4).Average(i => 0.2126 * bgra[i * 4 + 2] + 0.7152 * bgra[i * 4 + 1] + 0.0722 * bgra[i * 4]);
        var lumas = frames.Select(Luma).ToList();
        Assert.True(lumas.Max() - lumas.Min() < 20, string.Join(", ", lumas));
    }

    private static byte[] Pixels(MainViewModel model)
    {
        var frame = (WriteableBitmap)model.Preview!;
        using var locked = frame.Lock();
        var bytes = new byte[locked.RowBytes * locked.Size.Height];
        System.Runtime.InteropServices.Marshal.Copy(locked.Address, bytes, 0, bytes.Length);
        return bytes;
    }

    /// <summary>Uses a git-ignored RAW fixture when it is present.</summary>
    [AvaloniaFact]
    public async Task HDR_exports_write_an_ultra_hdr_jpeg_and_a_linear_exr()
    {
        var raw = Path.Combine(Root, "web/public/fixtures/real/DSC00395.ARW");
        if (!File.Exists(raw)) return;
        var (_, model) = await Open(raw);
        var settings = new OutputSettings { Format = "jpeg", Quality = 90, LongEdge = 1600, HdrHeadroom = 4 };
        var jpeg = await model.ExportToFolderAsync(settings, _folder);
        Assert.NotNull(jpeg);
        var bytes = await File.ReadAllBytesAsync(jpeg!);
        var text = System.Text.Encoding.Latin1.GetString(bytes);
        Assert.Contains("hdrgm:GainMapMax=\"2\"", text);
        Assert.Contains("Item:Semantic=\"GainMap\"", text);
        Assert.Contains("MPF\0", text);
        // Two pictures: the SDR one and, after it, the gain map.
        Assert.True(text.IndexOf("\xFF\xD8\xFF", 3, StringComparison.Ordinal) > 0);
        using (var decoded = new Avalonia.Media.Imaging.Bitmap(jpeg!)) Assert.Equal(1600, Math.Max(decoded.PixelSize.Width, decoded.PixelSize.Height));

        settings = new OutputSettings { Format = "exr", BitDepth = 16, LongEdge = 1600, HdrHeadroom = 4, ColorSpace = "p3" };
        var exr = await model.ExportToFolderAsync(settings, _folder);
        Assert.EndsWith(".exr", exr);
        var header = await File.ReadAllBytesAsync(exr!);
        Assert.Equal(new byte[] { 0x76, 0x2F, 0x31, 0x01 }, header[..4]);
        Assert.Contains("chromaticities", System.Text.Encoding.Latin1.GetString(header, 0, 600));
        if (Environment.GetEnvironmentVariable("OPENPIXELS_HDR_SAMPLES") is { Length: > 0 } samples)
        {
            Directory.CreateDirectory(samples);
            File.Copy(jpeg!, Path.Combine(samples, "ultra-hdr.jpg"), true);
            File.Copy(exr!, Path.Combine(samples, "linear.exr"), true);
        }
    }

    /// <summary>Uses git-ignored sample photos and SigLIP 2 when they are present.</summary>
    [AvaloniaFact]
    public async Task The_library_finds_photos_by_description_in_any_language_and_tags_them()
    {
        var names = new[] { "lite/lake.jpg", "lite/flowers.jpg", "lite/building.jpg", "lite/night-noise.jpg", "people/06-outdoor.jpg" };
        var paths = names.Select(name => Path.Combine(Root, "web/public/fixtures", name)).ToArray();
        if (!paths.All(File.Exists) || !ImageSearch.Available) return;
        var (_, model) = await Open(paths);
        await model.Analyze();
        PhotoItem Named(string name) => model.Photos.Single(photo => photo.Name == name);
        Assert.Contains("花", Named("flowers.jpg").TagText);
        Assert.Contains("湖泊", Named("lake.jpg").TagText);
        Assert.Contains("建筑", Named("building.jpg").TagText);
        Assert.Contains("夜景", Named("night-noise.jpg").TagText);

        async Task<string> Top(string query)
        {
            model.SearchText = query;
            await model.ApplySearchAsync();
            return model.VisiblePhotos[0].Name;
        }
        Assert.Equal("flowers.jpg", await Top("花"));
        Assert.Equal("flowers.jpg", await Top("flowers"));
        Assert.Equal("night-noise.jpg", await Top("夜晚的城市"));
        Assert.Equal("lake.jpg", await Top("湖と山"));
        Assert.Equal("06-outdoor.jpg", await Top("웃는 여자"));
        Assert.True(model.VisiblePhotos.Count < paths.Length, "a search hides what does not match");
        await Top("");
        Assert.Equal(paths.Length, model.VisiblePhotos.Count);

        // The tag filter lists the tags in use; choosing one shows only its photos.
        var flowers = model.TagFilters.ToList().FindIndex(name => name.StartsWith("花（", StringComparison.Ordinal));
        Assert.True(flowers > 0, string.Join(", ", model.TagFilters));
        model.TagFilterIndex = flowers;
        Assert.Equal(["flowers.jpg"], model.VisiblePhotos.Select(photo => photo.Name));
        model.TagFilterIndex = 0;
        Assert.Equal(paths.Length, model.VisiblePhotos.Count);
    }
}
