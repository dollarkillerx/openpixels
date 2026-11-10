using System.Text.Json.Nodes;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

public static class PhotoLoader
{
    /// <summary>Extensions offered when importing; the engine decides what it can really read.</summary>
    public static readonly string[] Extensions =
    [
        ".jpg", ".jpeg", ".png", ".tif", ".tiff", ".heic", ".heif",
        ".dng", ".cr2", ".cr3", ".nef", ".nrw", ".arw", ".raf", ".orf", ".rw2", ".pef", ".srw", ".iiq", ".3fr",
    ];

    public static bool IsPhoto(string path) => Extensions.Contains(Path.GetExtension(path).ToLowerInvariant());

    /// <summary>Decodes a file into the engine and returns what it reports about the image.</summary>
    public static async Task<JsonNode> DecodeAsync(EngineClient engine, string path)
    {
        var bytes = await File.ReadAllBytesAsync(path);
        await engine.Ready;
        if (PlatformDecoder.IsHeic(bytes))
        {
            var raster = await Task.Run(() => PlatformDecoder.Decode(bytes));
            // The original file follows the pixels, so its metadata can be kept.
            var data = new byte[raster.Rgba.Length + bytes.Length];
            raster.Rgba.CopyTo(data, 0);
            bytes.CopyTo(data, raster.Rgba.Length);
            return (await engine.CallAsync("decodeRgba", new { width = raster.Width, height = raster.Height }, data)).Json ?? new JsonObject();
        }
        return (await engine.CallAsync("decode", null, bytes)).Json ?? new JsonObject();
    }

    /// <summary>
    /// RAW files with no corrections of their own use the Lensfun database, if it knows the
    /// lens. Updates info's lens and adds lensSource.
    /// </summary>
    public static async Task ApplyLensDatabaseAsync(EngineClient engine, JsonNode info)
    {
        if (info["lens"] is not null || info["lensQuery"] is not { } query) return;
        if (Lensfun.Calibration(query) is not { } calibration) return;
        info["lens"] = (await engine.CallRawAsync("setLensCalibration", calibration)).Json?.DeepClone();
        info["lensSource"] = "lensfun";
    }
}

/// <summary>Turns engine errors, which are in English for developers, into something to show.</summary>
public static class Messages
{
    public static string Friendly(Exception exception)
    {
        var message = exception.Message;
        if (exception is FileNotFoundException or DirectoryNotFoundException) return "文件不存在或已被移动";
        if (exception is UnauthorizedAccessException) return "没有读取这个文件的权限";
        if (exception is IOException) return "读取文件失败";
        if (message.StartsWith("RAW decode failed", StringComparison.Ordinal)) return "无法解码这个 RAW 文件：相机型号可能还不支持，或文件已损坏";
        if (message.StartsWith("image decode failed", StringComparison.Ordinal) || message.Contains("unexpected pixel layout")) return "无法识别这个文件的格式，或文件已损坏";
        if (message.Contains("no image loaded")) return "还没有打开照片";
        // Messages written for the app itself are already in Chinese.
        if (message.Any(c => c is >= '一' and <= '鿿')) return message;
        return "处理时出错";
    }

    /// <summary>The original text, for a tooltip, when it is not already the friendly one.</summary>
    public static string? Detail(Exception exception) => Friendly(exception) == exception.Message ? null : exception.Message;
}

public sealed record HistoryEntry(string Label, Dictionary<string, double> Edit);

/// <summary>
/// One photo's edit history. Consecutive changes to the same settings within a moment
/// merge into one step, so a slider drag is undone in one go.
/// </summary>
public sealed class EditHistory
{
    private const int Limit = 100;
    private const long MergeMs = 1200;

    /// <summary>Keys the last unlabelled step changed; a quick step within them merges into it.</summary>
    private HashSet<string>? _lastKeys;
    private long _lastAt;

    private static HashSet<string> KeysOf(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after) =>
        Edits.ChangedKeys(before, after).Split(',', StringSplitOptions.RemoveEmptyEntries).ToHashSet();

    public EditHistory(IReadOnlyDictionary<string, double> opened)
    {
        Entries.Add(new("打开", new(opened)));
    }

    public List<HistoryEntry> Entries { get; } = [];
    public int Index { get; private set; }
    public bool CanUndo => Index > 0;
    public bool CanRedo => Index < Entries.Count - 1;
    public Dictionary<string, double> Current => new(Entries[Index].Edit);

    /// <summary>Records a change. label names an action; without one the change is described from its settings.</summary>
    public void Record(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after, string? label = null)
    {
        var keys = KeysOf(before, after);
        if (keys.Count == 0) return;
        var now = Environment.TickCount64;
        var merge = label is null && _lastKeys is not null && keys.IsSubsetOf(_lastKeys) && now - _lastAt < MergeMs && Index == Entries.Count - 1 && Index > 0;
        if (merge)
        {
            var basis = Entries[Index - 1].Edit;
            var merged = KeysOf(basis, after);
            if (merged.Count == 0)
            {
                // Dragged back to where it started: the step disappears.
                Entries.RemoveAt(Index--);
                _lastKeys = null;
                return;
            }
            Entries[Index] = new(Edits.DescribeChange(basis, after), new(after));
            _lastKeys!.UnionWith(merged);
        }
        else
        {
            Entries.RemoveRange(Index + 1, Entries.Count - Index - 1);
            Entries.Add(new(label ?? Edits.DescribeChange(before, after), new(after)));
            if (Entries.Count > Limit) Entries.RemoveAt(1);
            Index = Entries.Count - 1;
            _lastKeys = label is null ? keys : null;
        }
        _lastAt = now;
    }

    public Dictionary<string, double> MoveTo(int index)
    {
        Index = Math.Clamp(index, 0, Entries.Count - 1);
        _lastKeys = null;
        return Current;
    }
}
