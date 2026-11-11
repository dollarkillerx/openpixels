using Avalonia.Controls;
using Avalonia.Headless.XUnit;
using Avalonia.Media.Imaging;
using Xunit;
using static OpenPixels.Desktop.Tests.MainWindowTests;

namespace OpenPixels.Desktop.Tests;

public sealed class CullingLogicTests
{
    private static PhotoAnalysis Shot(double time, double sharpness, double shade, bool timed = true) =>
        new(time, timed, sharpness, 0, 0, false, Enumerable.Repeat(shade, 192).ToArray());

    [Fact]
    public void Photos_of_one_moment_group_together_and_the_sharpest_wins()
    {
        var items = new List<(string, PhotoAnalysis)>
        {
            ("a", Shot(0, 400, 0.5)), ("b", Shot(1000, 40, 0.51)), ("c", Shot(2000, 380, 0.5)),
            // Same look, but ten minutes later: another moment.
            ("d", Shot(600_000, 300, 0.5)),
            // Seconds later, but a different scene.
            ("e", Shot(601_000, 300, 0.9)),
        };
        var groups = Analysis.Group(items);
        Assert.Equal([["a", "b", "c"], ["d"], ["e"]], groups);
        var verdicts = Analysis.Judge(items.Take(3).ToList());
        Assert.True(verdicts["a"].Best);
        Assert.True(verdicts["b"].Reject, "ten times softer than the best");
        Assert.False(verdicts["c"].Reject, "nearly as sharp is kept");
    }

    [Fact]
    public void Without_capture_times_only_closer_looks_group()
    {
        var items = new List<(string, PhotoAnalysis)> { ("a", Shot(0, 1, 0.5, false)), ("b", Shot(0, 1, 0.54, false)) };
        Assert.Equal(2, Analysis.Group(items).Count);
    }

    [Fact]
    public void Capture_times_are_read_from_exif_text()
    {
        var first = Analysis.CaptureMillis("2024:05:01 10:00:00");
        var later = Analysis.CaptureMillis("2024:05:01 10:01:30");
        Assert.Equal(90_000, later - first);
        Assert.Null(Analysis.CaptureMillis("not a date"));
    }

    [Fact]
    public void Syncing_keeps_each_photos_own_framing()
    {
        var source = new Dictionary<string, double> { ["exposure"] = 1, ["cropW"] = 0.5, ["rotate"] = 3 };
        var target = new Dictionary<string, double> { ["contrast"] = 0.2, ["cropW"] = 0.8, ["local0Exposure"] = 0.4 };
        var merged = Edits.Merge(source, target);
        Assert.Equal(1, merged["exposure"]);
        Assert.Equal(0.8, merged["cropW"]);
        Assert.Equal(0.4, merged["local0Exposure"]);
        Assert.False(merged.ContainsKey("contrast"));
        Assert.False(merged.ContainsKey("rotate"));
    }
}

public sealed class LibraryTests : IDisposable
{
    private readonly string _folder = Directory.CreateTempSubdirectory("openpixels-library").FullName;

    public LibraryTests() => Environment.SetEnvironmentVariable("OPENPIXELS_DATA_DIR", Path.Combine(_folder, "data"));

    public void Dispose() => Directory.Delete(_folder, recursive: true);

    private static async Task<(MainWindow Window, MainViewModel Model)> Open(params string[] paths)
    {
        var window = new MainWindow { Width = 1400, Height = 880 };
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos(paths);
        await WaitFor(() => model.CanEdit && model.Preview is not null, "the preview", model);
        return (window, model);
    }

    private string[] ThreePhotos() =>
    [
        MakePhoto(_folder, "a.png"),
        MakePhoto(_folder, "b.png", solid: (200, 60, 60)),
        MakePhoto(_folder, "c.png", solid: (60, 60, 200)),
    ];

    [AvaloniaFact]
    public async Task Stars_flags_and_labels_filter_the_list_and_are_kept()
    {
        var paths = ThreePhotos();
        var (window, model) = await Open(paths);
        model.SetRating(3);
        model.SelectedPhoto = model.Photos[1];
        await WaitFor(() => model.CanEdit, "the second photo", model);
        model.SetFlag(CullStore.FlagReject);
        model.ToggleLabel(2);
        Assert.Equal("★★★", model.Photos[0].Stars);

        model.RatingFilterIndex = 3;
        Assert.Equal(["a.png"], model.VisiblePhotos.Select(photo => photo.Name));
        model.RatingFilterIndex = 0;
        model.FlagFilterIndex = 2; // Not rejected.
        Assert.Equal(["a.png", "c.png"], model.VisiblePhotos.Select(photo => photo.Name));
        model.FlagFilterIndex = 0;
        model.LabelFilterIndex = 2;
        Assert.Equal(["b.png"], model.VisiblePhotos.Select(photo => photo.Name));
        // Filtering the open photo out of the list does not close it.
        model.LabelFilterIndex = 1;
        Assert.Equal("b.png", model.SelectedPhoto?.Name);
        window.Close();

        var (_, again) = await Open(paths);
        Assert.Equal(3, again.Photos[0].Rating);
        Assert.True(again.Photos[1].IsRejected);
        Assert.Equal(2, again.Photos[1].Label);
    }

    [AvaloniaFact]
    public async Task Settings_sync_to_selected_photos_but_framing_stays()
    {
        var paths = ThreePhotos();
        var (_, model) = await Open(paths);
        model.SelectedPhoto = model.Photos[1];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto?.Name == "b.png", "b", model);
        model.CropRatioIndex = 1;
        model.SelectedPhoto = model.Photos[0];
        await WaitFor(() => model.CanEdit && model.CurrentEdit.Count == 0, "a", model);

        model.Slider("exposure").Value = 0.8;
        model.Slider("vibrance").Value = 0.3;
        if (!model.SelectedPhotos.Contains(model.Photos[0])) model.SelectedPhotos.Add(model.Photos[0]);
        model.SelectedPhotos.Add(model.Photos[1]);
        Assert.Equal("同步到选中（1）", model.SyncSelectedLabel);
        model.SyncToSelectedCommand.Execute(null);

        model.SelectedPhoto = model.Photos[1];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto?.Name == "b.png" && model.Slider("exposure").Value == 0.8, "b with the synced edit", model);
        Assert.Equal(0.3, model.Slider("vibrance").Value);
        Assert.True(model.CurrentEdit.ContainsKey("cropW"), "b keeps its own crop");
        // The sync is one undoable step on b.
        model.Undo();
        Assert.Equal(0, model.Slider("exposure").Value);

        model.SelectedPhoto = model.Photos[2];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto?.Name == "c.png", "c", model);
        Assert.Equal(0, model.Slider("exposure").Value);
    }

    [AvaloniaFact]
    public async Task Presets_and_copy_paste_carry_settings()
    {
        var paths = ThreePhotos();
        var (_, model) = await Open(paths);
        model.Slider("contrast").Value = 0.5;
        model.SelectedLook = model.LookChoices.Single(look => look.Id == 9);
        model.NewPresetName = "柔和";
        model.SavePresetCommand.Execute(null);
        model.CopySettingsCommand.Execute(null);

        model.SelectedPhoto = model.Photos[1];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto?.Name == "b.png", "b", model);
        model.SelectedPreset = "柔和";
        model.ApplyPresetCommand.Execute(null);
        Assert.Equal(0.5, model.Slider("contrast").Value);
        Assert.Equal(9, model.SelectedLook?.Id);
        Assert.Equal("预设 柔和", model.HistoryRows[^1].Label);

        model.SelectedPhoto = model.Photos[2];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto?.Name == "c.png", "c", model);
        model.PasteSettingsCommand.Execute(null);
        Assert.Equal(0.5, model.Slider("contrast").Value);
    }

    [AvaloniaFact]
    public async Task Smart_grouping_picks_the_sharp_shot_and_rejects_the_soft_one()
    {
        // The same scene twice, once with fine detail and once smooth, plus a different scene.
        byte Detail(int x, int y) => (byte)((x + y) % 2 == 0 ? 40 : 0);
        var sharp = MakePhoto(_folder, "1-sharp.png", pixel: (x, y) => ((byte)(x * 200 / 300 + Detail(x, y)), (byte)(y * 200 / 200 + Detail(x, y) / 2), 90));
        var soft = MakePhoto(_folder, "2-soft.png", pixel: (x, y) => ((byte)(x * 200 / 300 + 20), (byte)(y * 200 / 200 + 10), 90));
        var other = MakePhoto(_folder, "3-other.png", solid: (30, 160, 60));
        var (_, model) = await Open(sharp, soft, other);

        await model.Analyze();
        Assert.Equal(1, model.Photos[0].Group);
        Assert.Equal(1, model.Photos[1].Group);
        Assert.Equal(2, model.Photos[2].Group);
        Assert.True(model.Photos[0].IsBest);

        // Regrouping must not drop the open photo from the list's selection.
        Assert.Contains(model.SelectedPhoto!, model.SelectedPhotos);
        model.AutoPick();
        Assert.True(model.Photos[0].IsPicked);
        Assert.True(model.Photos[1].IsRejected);
        model.GroupFilterIndex = 1; // Best of each group.
        Assert.Equal(["1-sharp.png", "3-other.png"], model.VisiblePhotos.Select(photo => photo.Name));
    }

    [AvaloniaFact]
    public async Task A_batch_exports_every_photo_with_its_own_edit()
    {
        var paths = ThreePhotos();
        var (_, model) = await Open(paths);
        model.Slider("exposure").Value = 1;
        var output = Directory.CreateDirectory(Path.Combine(_folder, "out")).FullName;
        var settings = new OutputSettings { Format = "jpeg", Template = "trip-{n}", LongEdge = 1080 };
        var done = await model.ExportManyAsync([.. model.Photos], settings, output);
        Assert.Equal(3, done);
        Assert.Equal(["trip-001.jpg", "trip-002.jpg", "trip-003.jpg"], Directory.GetFiles(output).Select(Path.GetFileName).Order());
        // Only the first photo was brightened.
        static double Mean(string path)
        {
            using var bitmap = new Bitmap(path);
            var (width, height) = (bitmap.PixelSize.Width, bitmap.PixelSize.Height);
            var pixels = new byte[width * height * 4];
            unsafe
            {
                fixed (byte* buffer = pixels) bitmap.CopyPixels(new Avalonia.PixelRect(0, 0, width, height), (nint)buffer, pixels.Length, width * 4);
            }
            // Green is the second byte in both RGBA and BGRA.
            return pixels.Where((_, index) => index % 4 == 1).Average(value => (double)value);
        }
        Assert.True(Mean(Path.Combine(output, "trip-001.jpg")) > Mean(paths[0]) + 20);
    }

    [AvaloniaFact]
    public async Task Soft_proofing_uses_a_system_profile()
    {
        var (_, model) = await Open(MakePhoto(_folder, "colourful.png", pixel: (x, y) => ((byte)(x * 255 / 300), (byte)(255 - y), (byte)(255 - x * 255 / 300))));
        await WaitFor(() => model.ProofStatus.Length > 0, "the profile list", model);
        var cmyk = model.ProofProfiles.FirstOrDefault(profile => profile.Space == "CMYK");
        if (cmyk is null) return; // This system has no printer profile to proof with.
        var before = MeanGreen(model.Preview);
        model.SelectedProofProfile = cmyk;
        model.ProofGamut = true;
        model.ProofOn = true;
        Assert.StartsWith("软打样：", model.ProofBadge);
        await WaitFor(() => Math.Abs(MeanGreen(model.Preview) - before) > 2, "the proofed preview", model);
    }

    [AvaloniaFact]
    public async Task Projects_and_the_last_session_reopen_their_photos()
    {
        var paths = ThreePhotos();
        var (window, model) = await Open(paths[..2]);
        model.NewProjectName = "周末";
        model.SaveProjectCommand.Execute(null);
        window.Close();

        var fresh = new MainWindow();
        fresh.Show();
        var again = (MainViewModel)fresh.DataContext!;
        Assert.True(again.CanResume);
        Assert.Equal("继续上次的 2 张照片", again.ResumeLabel);
        Assert.Equal("周末", again.Projects.Single().Name);
        again.AddPhotos([paths[2]]);
        await WaitFor(() => again.CanEdit, "c", again);
        again.OpenProjectCommand.Execute(again.Projects.Single());
        Assert.Equal(["a.png", "b.png"], again.Photos.Select(photo => photo.Name));
        await WaitFor(() => again.CanEdit && again.SelectedPhoto?.Name == "a.png", "the project's first photo", again);
    }

    [AvaloniaFact]
    public async Task Grid_view_gives_the_window_to_the_photos()
    {
        var (window, model) = await Open(ThreePhotos());
        model.ToggleGrid();
        Assert.False(window.FindControl<Border>("DevelopPanel")!.IsVisible);
        Assert.Contains("grid", window.FindControl<ListBox>("Filmstrip")!.Classes);
        model.ToggleGrid();
        Assert.True(window.FindControl<Border>("DevelopPanel")!.IsVisible);
    }

    [AvaloniaFact]
    public async Task A_version_has_its_own_edit_exports_on_its_own_and_is_kept()
    {
        var path = MakePhoto(_folder, "scene.png", solid: (120, 120, 120));
        var (window, model) = await Open(path);
        model.Slider("exposure").Value = 0.5;
        model.CreateVersion();
        var version = model.SelectedPhoto!;
        Assert.True(version.IsVersion);
        Assert.Equal(2, model.Photos.Count);
        await WaitFor(() => model.CanEdit && model.CurrentEdit.GetValueOrDefault("exposure") == 0.5, "the version opened with the edit it came from", model);
        model.Slider("exposure").Value = -1;
        model.Slider("saturation").Value = -1;
        await WaitFor(() => model.CurrentEdit.GetValueOrDefault("saturation") == -1, "the version edited", model);

        // The photo itself kept its own edit.
        model.SelectedPhoto = model.Photos[0];
        await WaitFor(() => model.CanEdit && model.SelectedPhoto == model.Photos[0] && model.CurrentEdit.GetValueOrDefault("exposure") == 0.5, "the original edit", model);
        Assert.False(model.CurrentEdit.ContainsKey("saturation"));

        // Each exports under its own name.
        var output = Directory.CreateDirectory(Path.Combine(_folder, "out")).FullName;
        Assert.Equal("scene.jpg", Path.GetFileName(await model.ExportToFolderAsync(new OutputSettings(), output)));
        model.SelectedPhoto = version;
        await WaitFor(() => model.CanEdit && model.SelectedPhoto == version && model.CurrentEdit.GetValueOrDefault("exposure") == -1, "the version reopened", model);
        Assert.Equal("scene-v2.jpg", Path.GetFileName(await model.ExportToFolderAsync(new OutputSettings(), output)));
        window.Close();

        // A new window finds the version again beside its photo.
        var (again, reopened) = await Open(path);
        Assert.Equal(2, reopened.Photos.Count);
        Assert.True(reopened.Photos[1].IsVersion);
        reopened.SelectedPhoto = reopened.Photos[1];
        await WaitFor(() => reopened.CanEdit && reopened.SelectedPhoto == reopened.Photos[1] && reopened.CurrentEdit.GetValueOrDefault("exposure") == -1, "the kept version", reopened);
        Assert.True(reopened.CanDeleteVersion);
        reopened.DeleteVersion();
        Assert.Single(reopened.Photos);
        again.Close();
    }
}
