using System.Text.Json.Nodes;
using Avalonia.Headless.XUnit;
using Avalonia.Media.Imaging;
using Xunit;
using static OpenPixels.Desktop.Tests.MainWindowTests;

namespace OpenPixels.Desktop.Tests;

/// <summary>Pure logic shared with the web edition: it must give the same answers.</summary>
public sealed class SharedLogicTests
{
    private static string Root => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../.."));

    [Fact]
    public void The_spline_matches_the_shared_curve_cases()
    {
        var cases = JsonNode.Parse(File.ReadAllText(Path.Combine(Root, "crates/op-core/src/curve_cases.json")))!["cases"]!.AsArray();
        Assert.NotEmpty(cases);
        foreach (var item in cases)
        {
            var points = item!["points"]!.AsArray().Select(p => new CurvePoint(p![0]!.GetValue<double>(), p[1]!.GetValue<double>())).ToList();
            var amounts = item["parametric"]!.AsArray().Select(v => v!.GetValue<double>()).ToArray();
            var edit = new Dictionary<string, double>
            {
                ["curveHighlights"] = amounts[0], ["curveLights"] = amounts[1], ["curveDarks"] = amounts[2], ["curveShadows"] = amounts[3],
            };
            var spline = Curves.SplineThrough(points);
            var at = item["at"]!.AsArray().Select(v => v!.GetValue<double>()).ToArray();
            var expected = item["expected"]!.AsArray().Select(v => v!.GetValue<double>()).ToArray();
            for (var i = 0; i < at.Length; i++)
            {
                var value = Math.Clamp(spline(at[i]) + Curves.ParametricOffset(edit, at[i]), 0, 1);
                Assert.True(Math.Abs(value - expected[i]) < 1e-4, $"at {at[i]}: {value} vs {expected[i]}");
            }
        }
    }

    [Fact]
    public void Curve_points_round_trip_and_an_identity_curve_stores_nothing()
    {
        var edit = Curves.WithPoints(new Dictionary<string, double> { ["exposure"] = 1 }, "R", [new(0, 0), new(0.4, 0.55), new(1, 1)]);
        Assert.Equal(3, edit["curveRN"]);
        Assert.Equal(new CurvePoint(0.4, 0.55), Curves.PointsOf(edit, "R")[1]);
        var cleared = Curves.WithPoints(edit, "R", Curves.Identity);
        Assert.Equal(["exposure"], cleared.Keys);
    }

    /// <summary>Reference values computed by web/src/lut.ts, so a look id means the same in both editions.</summary>
    [Theory]
    [InlineData(1, new[] { 0.015, 0.693889, 0.11125, 0.403111, 0.554306, 0.95 })]
    [InlineData(3, new[] { 0, 0.681667, 0.14, 0.389374, 0.556667, 0.900244 })]
    [InlineData(4, new[] { 0.08, 0.639026, 0.207163, 0.474239, 0.540374, 0.94 })]
    [InlineData(8, new[] { 0, 0.68986, 0.171191, 0.461591, 0.579796, 0.985 })]
    [InlineData(10, new[] { 0.037, 0.668166, 0.1487, 0.414699, 0.545408, 0.941711 })]
    [InlineData(17, new[] { 0.015, 0.715912, 0, 0.364292, 0.698069, 1 })]
    [InlineData(22, new[] { 0, 0.556582, 0.25025, 0.661972, 0.502498, 1 })]
    [InlineData(24, new[] { 0.07, 0.564912, 0.319006, 0.647156, 0.514534, 0.95 })]
    public void Built_in_looks_match_the_web_edition(int id, double[] expected)
    {
        var lut = Looks.BuiltIn(id)!;
        int[] indices = [0, 1234, 7777, 20000, 31000, 46874];
        for (var i = 0; i < indices.Length; i++) Assert.True(Math.Abs(lut.Data[indices[i]] - expected[i]) < 2e-5, $"look {id} [{indices[i]}]: {lut.Data[indices[i]]} vs {expected[i]}");
    }

    [Fact]
    public void Film_looks_are_bundled_with_their_own_ids_and_keep_mid_grey()
    {
        Assert.True(Looks.FilmLooks.Count >= 20);
        Assert.Equal(Looks.FilmLooks.Count, Looks.FilmLooks.Select(look => look.Id).Distinct().Count());
        Assert.Empty(Looks.FilmLooks.Select(look => look.Id).Intersect(Looks.BuiltInLooks.Select(look => look.Id)));
        foreach (var look in Looks.FilmLooks)
        {
            Assert.InRange(look.Id, 30, Looks.FirstImportedId - 1);
            var lut = Looks.FilmLook(look.Id);
            Assert.NotNull(lut);
            // Negatives are balanced so mid grey keeps its brightness; slides keep their cast.
            var centre = lut.Size / 2;
            var at = ((centre * lut.Size + centre) * lut.Size + centre) * 3;
            var luma = 0.2126 * lut.Data[at] + 0.7152 * lut.Data[at + 1] + 0.0722 * lut.Data[at + 2];
            Assert.InRange(luma, 0.45, 0.55);
        }
        Assert.Contains(Looks.FilmLooks, look => look.Name == "Kodak Portra 400");
    }

    [Fact]
    public void Cube_files_are_parsed_and_bad_ones_explained()
    {
        var cube = "TITLE \"test\"\nLUT_3D_SIZE 2\n" + string.Join('\n', Enumerable.Range(0, 8).Select(i => $"{i & 1} {(i >> 1) & 1} {(i >> 2) & 1}"));
        var lut = Looks.ParseCube(cube);
        Assert.Equal(2, lut.Size);
        Assert.Equal(24, lut.Data.Length);
        Assert.Contains("1D", Assert.Throws<FormatException>(() => Looks.ParseCube("LUT_1D_SIZE 4\n0 0 0")).Message);
        Assert.Contains("不完整", Assert.Throws<FormatException>(() => Looks.ParseCube("LUT_3D_SIZE 2\n0 0 0")).Message);
    }

    [Fact]
    public void History_names_describe_the_change()
    {
        var before = new Dictionary<string, double> { ["exposure"] = 0.5 };
        Assert.Equal("曝光 +0.25", Edits.DescribeChange(before, new Dictionary<string, double> { ["exposure"] = 0.75 }));
        Assert.Equal("白平衡", Edits.DescribeChange(before, new Dictionary<string, double> { ["exposure"] = 0.5, ["temperature"] = 0.2, ["tint"] = -0.1 }));
        Assert.Equal("红色曲线", Edits.DescribeChange(new Dictionary<string, double>(), Curves.WithPoints(new Dictionary<string, double>(), "R", [new(0, 0), new(0.5, 0.6), new(1, 1)])));
        Assert.Equal("阴影色轮", Edits.DescribeChange(new Dictionary<string, double>(), new Dictionary<string, double> { ["gradeShadowHue"] = 200, ["gradeShadowAmount"] = 0.3 }));
    }

    [Fact]
    public void A_slider_drag_is_one_history_step_and_undo_goes_back_over_it()
    {
        var history = new EditHistory(new Dictionary<string, double>());
        var edit = new Dictionary<string, double>();
        foreach (var value in new[] { 0.1, 0.2, 0.3 })
        {
            var next = new Dictionary<string, double>(edit) { ["exposure"] = value };
            history.Record(edit, next);
            edit = next;
        }
        var contrast = new Dictionary<string, double>(edit) { ["contrast"] = 0.2 };
        history.Record(edit, contrast);
        Assert.Equal(["打开", "曝光 +0.30", "对比度 +0.20"], history.Entries.Select(entry => entry.Label));
        Assert.Equal(0.3, history.MoveTo(1)["exposure"]);
        Assert.False(history.MoveTo(0).ContainsKey("exposure"));
    }

    [Fact]
    public void Editing_after_an_undo_starts_a_branch_and_keeps_the_old_steps()
    {
        var history = new EditHistory(new Dictionary<string, double>());
        Dictionary<string, double> Step(string key, double value, Dictionary<string, double> from)
        {
            var next = new Dictionary<string, double>(from) { [key] = value };
            history.Record(from, next, key);
            return next;
        }
        var exposure = Step("exposure", 0.5, []);
        Step("contrast", 0.3, exposure);
        // Back to the exposure step, then a different change: the contrast step stays.
        history.MoveTo(history.UndoTarget);
        var branch = Step("saturation", 0.4, exposure);
        Assert.Equal(4, history.Entries.Count);
        Assert.True(history.HasBranches);
        Assert.Equal(["opened", "exposure", "contrast", "saturation"], history.Lines().Select(line => line.Index switch { 0 => "opened", var i => history.Entries[i].Label }));
        Assert.Equal([0, 0, 0, 1], history.Lines().Select(line => line.Depth));
        Assert.Equal([true, true, false, true], history.Lines().Select(line => line.OnPath));
        // Undo follows the branch back; redo returns along it.
        Assert.Equal(1, history.UndoTarget);
        history.MoveTo(1);
        Assert.Equal(3, history.RedoTarget);
        // The old branch can be visited, and redo then follows it.
        Assert.Equal(0.3, history.MoveTo(2)["contrast"]);
        history.MoveTo(1);
        Assert.Equal(2, history.RedoTarget);
        Assert.False(history.MoveTo(3).ContainsKey("contrast"));
        Assert.Equal(0.4, branch["saturation"]);
    }

    [Fact]
    public void Export_names_follow_the_template_and_never_collide()
    {
        var used = new HashSet<string> { "lake.jpg" };
        Assert.Equal("lake-2.jpg", Output.ExportName("lake.NEF", used, "{name}", 1, "jpg"));
        Assert.Equal("trip_007.png", Output.ExportName("x.jpg", used, "trip/{n}", 7, "png"));
    }

    [Fact]
    public void Lens_names_are_normalised_like_the_web_edition()
    {
        Assert.Equal("2470mmf2.8dgdna", Lensfun.Normalize("24-70mm F2.8 DG DN | Art"));
        Assert.Equal("z2470mmf4s", Lensfun.Normalize("NIKON Z 24-70mm f/4 S"));
    }

    [Fact]
    public void A_crop_is_shrunk_to_stay_inside_a_rotated_image()
    {
        var crop = Frame.ShrinkToImage(Crop.Full, 10, 3000, 2000);
        Assert.True(crop.W < 1 && crop.H < 1);
        Assert.Equal(0.5, crop.X + crop.W / 2, 3);
        var locked = Frame.DragCrop(Crop.Full, CropHandle.SouthEast, -0.5, -0.1, 1, 3000, 2000);
        Assert.Equal(1, locked.W * 3000 / (locked.H * 2000), 3);
    }
}

public sealed class FeatureTests : IDisposable
{
    private readonly string _folder = Directory.CreateTempSubdirectory("openpixels-features").FullName;

    public FeatureTests() => Environment.SetEnvironmentVariable("OPENPIXELS_DATA_DIR", Path.Combine(_folder, "data"));

    public void Dispose() => Directory.Delete(_folder, recursive: true);

    private static string Root => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../.."));

    private static async Task<MainViewModel> Open(params string[] paths)
    {
        var window = new MainWindow { Width = 1400, Height = 880 };
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos(paths);
        await WaitFor(() => model.CanEdit && model.Preview is not null, "the preview", model);
        return model;
    }

    [AvaloniaFact]
    public async Task Undo_and_redo_move_through_the_history()
    {
        var model = await Open(MakePhoto(_folder));
        var exposure = model.Slider("exposure");
        exposure.Value = 1;
        model.Slider("contrast").Value = 0.3;
        Assert.Equal(3, model.HistoryRows.Count);
        model.Undo();
        Assert.Equal(0, model.Slider("contrast").Value);
        Assert.Equal(1, exposure.Value);
        model.Undo();
        Assert.Equal(0, exposure.Value);
        model.Redo();
        Assert.Equal(1, exposure.Value);
        // Choosing an entry in the history list jumps to it.
        model.HistoryIndex = 2;
        Assert.Equal(0.3, model.Slider("contrast").Value);
    }

    [AvaloniaFact]
    public async Task Snapshots_are_kept_and_restore_the_edit()
    {
        var model = await Open(MakePhoto(_folder));
        model.Slider("saturation").Value = -1;
        model.TakeSnapshotCommand.Execute(null);
        model.ResetCommand.Execute(null);
        Assert.Equal(0, model.Slider("saturation").Value);
        model.ApplySnapshotCommand.Execute(model.Snapshots.Single());
        Assert.Equal(-1, model.Slider("saturation").Value);
    }

    [AvaloniaFact]
    public async Task A_look_changes_the_preview_and_a_failed_open_clears_it()
    {
        var model = await Open(MakePhoto(_folder));
        var before = MeanGreen(model.Preview);
        model.SelectedLook = model.LookChoices.Single(look => look.Name == "赛博朋克");
        Assert.Equal(1, model.LookAmount.Value);
        await WaitFor(() => Math.Abs(MeanGreen(model.Preview) - before) > 3, "the look in the preview", model);

        var broken = Path.Combine(_folder, "broken.png");
        File.WriteAllText(broken, "nope");
        model.AddPhotos([broken]);
        await WaitFor(() => model.Status.StartsWith("无法打开"), "the error", model);
        Assert.Null(model.Preview);
        Assert.Null(model.Histogram);
    }

    /// <summary>Cyberpunk turns greens towards cyan in web/src/lut.ts; they must not come out magenta.</summary>
    [AvaloniaFact]
    public async Task A_look_keeps_its_colours()
    {
        var model = await Open(MakePhoto(_folder, "green.png", solid: (64, 140, 50)));
        model.SelectedLook = model.LookChoices.Single(look => look.Id == 17);
        await Task.Delay(500);
        await WaitFor(() => CentrePixel(model.Preview) is var (r, g, _) && (r, g) != (0, 0), "a frame", model);
        var (red, green, blue) = CentrePixel(model.Preview);
        Assert.True(green > red && green > 100, $"rgb {red},{green},{blue}");
    }

    private static (int R, int G, int B) CentrePixel(Bitmap? bitmap)
    {
        var frame = (WriteableBitmap)bitmap!;
        using var locked = frame.Lock();
        var at = locked.Address + locked.Size.Height / 2 * locked.RowBytes + locked.Size.Width / 2 * 4;
        var pixel = new byte[4];
        System.Runtime.InteropServices.Marshal.Copy(at, pixel, 0, 4);
        return (pixel[0], pixel[1], pixel[2]);
    }

    [AvaloniaFact]
    public async Task Photos_get_thumbnails_without_being_opened()
    {
        var first = MakePhoto(_folder, "a.png");
        var second = MakePhoto(_folder, "b.png", 200, 300);
        var model = await Open(first, second);
        Assert.Equal("a.png", model.SelectedPhoto!.Name);
        await WaitFor(() => model.Photos[1].Thumbnail is not null, "the second thumbnail", model);
        Assert.True(model.Photos[1].Thumbnail!.PixelSize.Height > model.Photos[1].Thumbnail!.PixelSize.Width);
    }

    [AvaloniaFact]
    public async Task The_histogram_zoom_and_tools_work()
    {
        // Larger than the stage, so 100% is a real zoom.
        var model = await Open(MakePhoto(_folder, "large.png", 3000, 2000));
        await WaitFor(() => model.Histogram is not null, "the histogram", model);
        Assert.True(model.Histogram!.Total > 1000);

        model.ToggleZoom();
        Assert.True(model.IsZoomed);
        Assert.Equal("100%", model.ZoomLabel);
        model.ToggleZoom();
        Assert.False(model.IsZoomed);

        // Clicking the middle of a gradient picks a colour for the colour mixer.
        model.PickPointColourCommand.Execute(null);
        var (width, height) = model.Viewport;
        await model.PickAtAsync(width / 2.0, height / 2.0);
        Assert.True(model.PointActive, model.ToolHint);
        Assert.Equal(Picker.None, model.Picker);

        model.SetGrade(model.Grades[0], 210, 0.4);
        Assert.Equal(0.4, model.CurrentEdit["gradeShadowAmount"]);
        Assert.Equal("阴影色轮", model.HistoryRows[^1].Label);
    }

    [AvaloniaFact]
    public async Task Crop_and_watermark_reach_the_exported_file()
    {
        var model = await Open(MakePhoto(_folder));
        model.CropRatioIndex = 1; // 1:1
        var settings = new OutputSettings { Format = "png", Template = "{name}-square", Watermark = new WatermarkSettings { Enabled = true, Text = "OpenPixels", Size = 0.08, Opacity = 1 } };
        var path = await model.ExportToFolderAsync(settings, _folder);
        Assert.NotNull(path);
        Assert.Equal("gradient-square.png", Path.GetFileName(path));
        using var exported = new Bitmap(path!);
        Assert.Equal(200, exported.PixelSize.Width);
        Assert.Equal(200, exported.PixelSize.Height);
        // A second export does not overwrite the first.
        Assert.Equal("gradient-square-2.png", Path.GetFileName(await model.ExportToFolderAsync(settings, _folder)));
    }

    [AvaloniaFact]
    public async Task A_folder_is_imported_in_name_order()
    {
        var folder = Directory.CreateDirectory(Path.Combine(_folder, "shoot")).FullName;
        MakePhoto(folder, "b.png");
        MakePhoto(folder, "a.png");
        File.WriteAllText(Path.Combine(folder, "notes.txt"), "skip me");
        var window = new MainWindow();
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        Assert.Equal(2, model.AddFolder(folder));
        Assert.Equal(["a.png", "b.png"], model.Photos.Select(photo => photo.Name));
    }

    /// <summary>Uses the git-ignored HEIC fixture when this machine has one and can decode HEIC.</summary>
    [AvaloniaFact]
    public async Task Opens_a_heic_photo_through_the_system_decoder()
    {
        var heic = Path.Combine(Root, "web/public/fixtures/portrait/portrait.heic");
        if (!File.Exists(heic) || !Engine.PlatformDecoder.CanDecodeHeic) return;
        var model = await Open(heic);
        Assert.Contains("portrait.heic", model.Status);
    }

    /// <summary>The Nikon fixture's lens is in the Lensfun database, when both are present.</summary>
    [AvaloniaFact]
    public async Task A_raw_without_lens_data_uses_the_lens_database()
    {
        var raw = Path.Combine(Root, "web/public/fixtures/real/3-Nikon-Z7-RAW-14bit-lossless-compressed-L.NEF");
        if (!File.Exists(raw) || !File.Exists(Lensfun.DatabasePath)) return;
        var model = await Open(raw);
        Assert.True(model.LensAutoAvailable, model.LensStatus);
        model.LensAuto = true;
        Assert.Equal(1, model.CurrentEdit["lensAuto"]);
    }
}
