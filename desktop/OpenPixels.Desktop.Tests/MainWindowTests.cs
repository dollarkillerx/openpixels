using Avalonia;
using Avalonia.Headless;
using Avalonia.Headless.XUnit;
using Avalonia.Media.Imaging;
using Avalonia.Platform;
using OpenPixels.Desktop;
using Xunit;

[assembly: AvaloniaTestApplication(typeof(OpenPixels.Desktop.Tests.TestApp))]

namespace OpenPixels.Desktop.Tests;

public static class TestApp
{
    // Real Skia drawing, so a test can capture what the window looks like.
    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>().UseSkia().UseHeadless(new AvaloniaHeadlessPlatformOptions { UseHeadlessDrawing = false });
}

public sealed class MainWindowTests : IDisposable
{
    private readonly string _folder = Directory.CreateTempSubdirectory("openpixels-tests").FullName;

    public MainWindowTests() => Environment.SetEnvironmentVariable("OPENPIXELS_DATA_DIR", Path.Combine(_folder, "data"));

    public void Dispose() => Directory.Delete(_folder, recursive: true);

    /// <summary>A colourful gradient saved as PNG, so the test needs no fixture files.
    /// Encoded here because the headless platform cannot encode bitmaps.</summary>
    internal static string MakePhoto(string folder, string name = "gradient.png", int width = 300, int height = 200, (byte R, byte G, byte B)? solid = null, Func<int, int, (byte R, byte G, byte B)>? pixel = null)
    {
        var raw = new MemoryStream();
        for (var y = 0; y < height; y++)
        {
            raw.WriteByte(0); // No filter.
            for (var x = 0; x < width; x++)
            {
                var (r, g, b) = pixel?.Invoke(x, y) ?? solid ?? ((byte)(x * 255 / width), (byte)(y * 255 / height), (byte)90);
                raw.WriteByte(r);
                raw.WriteByte(g);
                raw.WriteByte(b);
            }
        }
        var compressed = new MemoryStream();
        using (var zlib = new System.IO.Compression.ZLibStream(compressed, System.IO.Compression.CompressionLevel.Fastest, leaveOpen: true))
        {
            raw.Position = 0;
            raw.CopyTo(zlib);
        }
        var png = new MemoryStream();
        png.Write([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        var header = new byte[13];
        System.Buffers.Binary.BinaryPrimitives.WriteInt32BigEndian(header, width);
        System.Buffers.Binary.BinaryPrimitives.WriteInt32BigEndian(header.AsSpan(4), height);
        header[8] = 8; // Bits per channel.
        header[9] = 2; // RGB.
        WriteChunk(png, "IHDR", header);
        WriteChunk(png, "IDAT", compressed.ToArray());
        WriteChunk(png, "IEND", []);
        var path = Path.Combine(folder, name);
        File.WriteAllBytes(path, png.ToArray());
        return path;
    }

    private static void WriteChunk(Stream stream, string type, byte[] data)
    {
        var length = new byte[4];
        System.Buffers.Binary.BinaryPrimitives.WriteInt32BigEndian(length, data.Length);
        stream.Write(length);
        var body = System.Text.Encoding.ASCII.GetBytes(type).Concat(data).ToArray();
        stream.Write(body);
        var crc = new byte[4];
        System.Buffers.Binary.BinaryPrimitives.WriteUInt32BigEndian(crc, Crc32(body));
        stream.Write(crc);
    }

    private static uint Crc32(byte[] bytes)
    {
        var crc = 0xFFFFFFFFu;
        foreach (var b in bytes)
        {
            crc ^= b;
            for (var k = 0; k < 8; k++) crc = (crc & 1) != 0 ? 0xEDB88320u ^ (crc >> 1) : crc >> 1;
        }
        return ~crc;
    }

    internal static async Task WaitFor(Func<bool> condition, string what, MainViewModel? model = null)
    {
        for (var i = 0; i < 400 && !condition(); i++) await Task.Delay(50);
        Assert.True(condition(), $"timed out waiting for {what}; status: {model?.Status}");
    }

    internal static double MeanGreen(Bitmap? bitmap)
    {
        Assert.NotNull(bitmap);
        var frame = (WriteableBitmap)bitmap!;
        using var locked = frame.Lock();
        var bytes = new byte[locked.RowBytes * locked.Size.Height];
        System.Runtime.InteropServices.Marshal.Copy(locked.Address, bytes, 0, bytes.Length);
        double sum = 0;
        for (var i = 1; i < bytes.Length; i += 4) sum += bytes[i];
        return sum / (bytes.Length / 4.0);
    }

    [AvaloniaFact]
    public async Task Opens_edits_and_exports_a_photo()
    {
        var window = new MainWindow();
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos([MakePhoto(_folder)]);

        await WaitFor(() => model.CanEdit && model.Preview is not null, "the preview", model);
        await WaitFor(() => model.Photos[0].Thumbnail is not null, "the thumbnail");
        var before = MeanGreen(model.Preview);

        var exposure = model.Groups.SelectMany(group => group.Sliders).Single(slider => slider.Spec.Key == "exposure");
        exposure.Value = 1.5;
        await WaitFor(() => MeanGreen(model.Preview) > before + 10, "a brighter preview");

        var exported = Path.Combine(_folder, "out.jpg");
        await model.ExportAsync(exported);
        Assert.True(File.Exists(exported), model.Status);
        var head = File.ReadAllBytes(exported)[..2];
        Assert.Equal(new byte[] { 0xFF, 0xD8 }, head);
    }

    [AvaloniaFact]
    public async Task Edits_are_kept_for_the_next_session()
    {
        var path = MakePhoto(_folder);
        var first = new MainWindow();
        first.Show();
        var model = (MainViewModel)first.DataContext!;
        model.AddPhotos([path]);
        await WaitFor(() => model.CanEdit, "the photo to open", model);
        model.Groups.SelectMany(group => group.Sliders).Single(slider => slider.Spec.Key == "contrast").Value = 0.4;
        first.Close();

        var second = new MainWindow();
        second.Show();
        var again = (MainViewModel)second.DataContext!;
        again.AddPhotos([path]);
        await WaitFor(() => again.CanEdit, "the photo to reopen");
        Assert.Equal(0.4, again.Groups.SelectMany(group => group.Sliders).Single(slider => slider.Spec.Key == "contrast").Value, 3);
    }

    /// <summary>Uses a real RAW from the git-ignored fixtures when this machine has one.</summary>
    [AvaloniaFact]
    public async Task Develops_and_exports_a_real_raw_at_full_size()
    {
        var root = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../.."));
        var raw = Path.Combine(root, "web/public/fixtures/real/3-Nikon-Z7-RAW-14bit-lossless-compressed-L.NEF");
        if (!File.Exists(raw)) return;
        var window = new MainWindow();
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos([raw]);
        await WaitFor(() => model.CanEdit && model.Preview is not null, "the RAW preview", model);
        var exported = Path.Combine(_folder, "raw.jpg");
        await model.ExportAsync(exported);
        Assert.True(File.Exists(exported), model.Status);
        Assert.True(new FileInfo(exported).Length > 2_000_000, $"a full-size JPEG: {model.Status}");
    }

    [AvaloniaFact]
    public async Task A_file_the_engine_cannot_read_reports_why()
    {
        var bad = Path.Combine(_folder, "broken.jpg");
        File.WriteAllText(bad, "not an image");
        var window = new MainWindow();
        window.Show();
        var model = (MainViewModel)window.DataContext!;
        model.AddPhotos([bad]);
        await WaitFor(() => model.Status.StartsWith("无法打开"), "an error message");
        Assert.False(model.CanEdit);
        Assert.Null(model.Preview);
        // The engine's English wording goes to the tooltip, not the status line.
        Assert.DoesNotContain("decode", model.Status);
        Assert.Contains("decode", model.StatusDetail);
    }
}
