using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Headless.XUnit;
using Avalonia.Threading;
using Avalonia.VisualTree;
using Xunit;

namespace OpenPixels.Desktop.Tests;

/// <summary>
/// Walks through the app with real photos and saves a screenshot at each step, for a
/// person to look at. Runs only when OPENPIXELS_TOUR names a folder for the pictures.
/// </summary>
public sealed class UiTour
{
    private static string? Folder => Environment.GetEnvironmentVariable("OPENPIXELS_TOUR");

    private static async Task WaitFor(Func<bool> condition, int tries = 600)
    {
        for (var i = 0; i < tries && !condition(); i++) await Task.Delay(50);
    }

    private static async Task Shot(Window window, string name)
    {
        await Task.Delay(400);
        Dispatcher.UIThread.RunJobs();
        window.CaptureRenderedFrame()?.Save(Path.Combine(Folder!, name + ".png"));
    }

    /// <summary>Opens only the named panel sections, scrolled into view.</summary>
    private static void Expand(Window window, params string[] headers)
    {
        foreach (var expander in window.GetVisualDescendants().OfType<Expander>())
        {
            expander.IsExpanded = headers.Contains(expander.Header as string);
        }
        Dispatcher.UIThread.RunJobs();
        window.GetVisualDescendants().OfType<Expander>().FirstOrDefault(expander => expander.Header as string == headers[0])?.BringIntoView();
    }

    private static void ShowTab(Window window, int index) => window.GetVisualDescendants().OfType<TabControl>().First().SelectedIndex = index;

    [AvaloniaFact]
    public async Task Tour()
    {
        if (Folder is null) return;
        Directory.CreateDirectory(Folder);
        Environment.SetEnvironmentVariable("OPENPIXELS_DATA_DIR", Path.Combine(Folder, "data"));
        var root = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../.."));
        var photos = new[]
        {
            "web/public/fixtures/lite/lake.jpg",
            "web/public/fixtures/people/05-woman.jpg",
            "web/public/fixtures/real/3-Nikon-Z7-RAW-14bit-lossless-compressed-L.NEF",
            "web/public/fixtures/portrait/portrait.heic",
        }.Select(path => Path.Combine(root, path)).Where(File.Exists).ToArray();

        var window = new MainWindow { Width = 1440, Height = 900 };
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        await Shot(window, "01-start");

        model.AddPhotos(photos);
        await WaitFor(() => model.CanEdit && model.Preview is not null && model.Histogram is not null);
        await WaitFor(() => model.Photos.All(photo => photo.Thumbnail is not null), 300);
        await Shot(window, "02-opened");

        Expand(window, "曲线");
        model.SetCurve(Curves.WithPoints(model.CurrentEdit, "M", [new(0, 0.04), new(0.25, 0.2), new(0.75, 0.82), new(1, 1)]));
        await Shot(window, "03-curve");

        Expand(window, "HSL", "色调分离");
        model.HslBand = 5;
        model.Slider("hslSat5").Value = 0.5;
        model.Slider("hslLum5").Value = -0.3;
        model.SetGrade(model.Grades[0], 200, 0.35);
        model.SetGrade(model.Grades[2], 40, 0.25);
        await Shot(window, "04-hsl-grading");

        Expand(window, "风格滤镜");
        model.SelectedLook = model.LookChoices.Single(look => look.Name == "赛博朋克");
        await Shot(window, "05-look");

        Expand(window, "裁剪与旋转");
        model.CropEditing = true;
        model.CropRatioIndex = 2; // 4:5
        model.RotateSlider.Value = 4;
        await Shot(window, "06-crop-editing");
        model.CropEditing = false;
        await Shot(window, "07-cropped");

        model.CompareOn = true;
        await Shot(window, "08-compare");
        model.CompareOn = false;

        ShowTab(window, 1);
        await Shot(window, "09-history");
        ShowTab(window, 0);

        model.SelectedPhoto = model.Photos[2 % model.Photos.Count];
        await WaitFor(() => model.CanEdit && model.Status.Contains("ms"));
        Expand(window, "相机与镜头");
        await Shot(window, "10-raw-lens");
        model.ZoomToRatio(1);
        await WaitFor(() => model.Status.Contains("全尺寸细节") && !model.Status.StartsWith("正在"), 900);
        await Task.Delay(800);
        await Shot(window, "11-raw-100");
        model.FitViewCommand.Execute(null);

        if (model.Photos.Count > 3)
        {
            model.SelectedPhoto = model.Photos[3];
            await WaitFor(() => model.Status.StartsWith("无法打开") || (model.CanEdit && model.Preview is not null));
            await Shot(window, "12-heic");
        }

        // Batch 2: culling, grouping, grid view, presets, soft proofing, projects.
        model.SelectedPhoto = model.Photos[0];
        await WaitFor(() => model.CanEdit && model.Preview is not null);
        model.SetRating(4);
        model.SetFlag(CullStore.FlagPick);
        model.ToggleLabel(3);
        model.SelectedPhoto = model.Photos[1];
        await WaitFor(() => model.CanEdit && model.Preview is not null);
        model.SetRating(2);
        model.SetFlag(CullStore.FlagReject);
        await model.Analyze();
        await Shot(window, "13-culling");
        model.ToggleGrid();
        await Shot(window, "14-grid");
        model.ToggleGrid();

        model.SelectedPhoto = model.Photos[0];
        await WaitFor(() => model.CanEdit && model.Preview is not null);
        Expand(window, "软打样", "预设与同步");
        await WaitFor(() => model.ProofStatus.Length > 0);
        model.SelectedProofProfile = model.ProofProfiles.FirstOrDefault(profile => profile.Space == "CMYK");
        model.ProofGamut = true;
        model.ProofOn = true;
        await Task.Delay(1200);
        await Shot(window, "15-soft-proof");
        File.WriteAllText(Path.Combine(Folder, "proof.txt"), $"{model.ProofStatus} {model.ProofBadge}\n" + string.Join("\n", model.ProofProfiles));
        model.ProofOn = false;

        model.NewProjectName = "巡检";
        model.SaveProjectCommand.Execute(null);
        ShowTab(window, 3);
        await Shot(window, "16-projects");
        ShowTab(window, 0);

        var export = new ExportWindow(model.Output, 2, model.Photos.Count);
        export.Show();
        await Shot(export, "17-export-dialog");
        export.Close();

        // How fast the preview follows a slider on the RAW: each change waits for the next frame.
        model.SelectedPhoto = model.Photos[2 % model.Photos.Count];
        await WaitFor(() => model.CanEdit && model.Status.Contains("ms"));
        await Task.Delay(500);
        var timings = new List<long>();
        var exposure = model.Slider("exposure");
        foreach (var value in new[] { 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0 })
        {
            var before = model.Preview;
            var watch = System.Diagnostics.Stopwatch.StartNew();
            exposure.Value = value;
            await WaitFor(() => !ReferenceEquals(model.Preview, before));
            timings.Add(watch.ElapsedMilliseconds);
        }
        File.WriteAllText(Path.Combine(Folder, "frame-ms.txt"), string.Join(",", timings));

        window.Width = 1000;
        window.Height = 640;
        await Task.Delay(800);
        await Shot(window, "18-small-window");
        File.WriteAllText(Path.Combine(Folder, "status.txt"), model.Status);
    }
}
