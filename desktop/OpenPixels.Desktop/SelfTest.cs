using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

/// <summary>
/// `OpenPixels --self-test` checks a packaged build without opening a window: the native
/// engine loads and renders, and the bundled models and lens database are there and load.
/// Prints one line per check and exits non-zero if any fails.
/// </summary>
public static class SelfTest
{
    public static int Run()
    {
        var failed = 0;
        void Check(string name, Func<string> check)
        {
            try
            {
                Console.WriteLine($"ok    {name}: {check()}");
            }
            catch (Exception exception)
            {
                failed++;
                Console.WriteLine($"FAIL  {name}: {exception.GetType().Name}: {exception.Message}");
            }
        }

        Console.WriteLine($"OpenPixels {typeof(SelfTest).Assembly.GetName().Version} in {AppContext.BaseDirectory}");
        using var engine = new EngineClient();
        Check("engine", () =>
        {
            engine.Ready.GetAwaiter().GetResult();
            const int width = 64, height = 48;
            var rgba = new byte[width * height * 4];
            for (var i = 0; i < width * height; i++) (rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2], rgba[i * 4 + 3]) = ((byte)(i % width * 4), 120, 60, 255);
            engine.CallAsync("decodeRgba", new { width, height }, rgba).GetAwaiter().GetResult();
            engine.CallAsync("buildProxy", new { maxLongEdge = 64 }).GetAwaiter().GetResult();
            engine.CallRawAsync("setAdjustments", Edits.ToEngineJson(new Dictionary<string, double> { ["exposure"] = 0.5 })).GetAwaiter().GetResult();
            var frame = engine.CallAsync("render", new { width = 32, height = 24 }).GetAwaiter().GetResult().Data;
            var jpeg = engine.CallAsync("exportImage", new { settings = new { format = "jpeg", quality = 90, longEdge = 0 } }).GetAwaiter().GetResult().Data;
            if (frame.Length != 32 * 24 * 4 || jpeg.Length < 100 || jpeg[0] != 0xFF) throw new InvalidOperationException("unexpected output");
            return $"rendered and exported a {jpeg.Length}-byte JPEG";
        });
        using var models = new AiModels();
        foreach (var file in new[] { AiModels.Matte.File, AiModels.Sky.File, AiModels.Inpaint.File })
        {
            Check(file, () =>
            {
                if (!AiModels.Has(file)) throw new FileNotFoundException("not bundled");
                if (file == AiModels.Matte.File)
                {
                    var (width, height) = AiModels.Matte.InputSize(64, 48);
                    var matte = models.SubjectMatte(new float[3 * width * height], width, height);
                    return $"ran on {width}x{height}, {matte.Length} values";
                }
                return $"{new FileInfo(Path.Combine(AiModels.Folder, file)).Length / 1048576} MB";
            });
        }
        Check("sam", () =>
        {
            if (!AiModels.Segment.Available) throw new FileNotFoundException("not bundled");
            const int size = AiModels.Segment.Size;
            var features = models.EncodeForSegment(new float[3 * size * size]);
            var logits = models.SegmentLogits(features, [(0.5, 0.5, true)], null);
            return $"encoded and decoded one click, {(logits is null ? "no object" : $"{logits.Length} values")}";
        });
        Check("faces", () =>
        {
            if (!FaceFinder.Available) throw new FileNotFoundException("not bundled");
            using var finder = new FaceFinder();
            var found = finder.Find(new byte[64 * 48 * 3], 64, 48);
            return $"ran on an empty frame, {found.Count} faces";
        });
        Check("heic", () => PlatformDecoder.CanDecodeHeic ? (OperatingSystem.IsMacOS() ? "system decoder" : "libheif")
            : OperatingSystem.IsLinux() ? "needs the system's libheif1" : throw new DllNotFoundException("libheif not bundled"));
        Check("lensfun", () => File.Exists(Lensfun.DatabasePath) ? $"{new FileInfo(Lensfun.DatabasePath).Length / 1024} KB" : throw new FileNotFoundException("not bundled"));
        Check("mcp", () =>
        {
            var server = Path.Combine(AppContext.BaseDirectory, OperatingSystem.IsWindows() ? "openpixels-mcp.exe" : "openpixels-mcp");
            if (!File.Exists(server)) throw new FileNotFoundException("not bundled");
            using var process = System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(server, "--tools") { RedirectStandardOutput = true, UseShellExecute = false })!;
            var tools = System.Text.Json.JsonDocument.Parse(process.StandardOutput.ReadToEnd()).RootElement.GetArrayLength();
            process.WaitForExit();
            return $"{tools} tools";
        });
        Check("languages", () => string.Join(", ", I18n.Languages.Select(language => language.Code)) + $"; using {I18n.Language}");
        Console.WriteLine(failed == 0 ? "all checks passed" : $"{failed} check(s) failed");
        return failed == 0 ? 0 : 1;
    }
}
