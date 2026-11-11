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
            // The colour profile, then the original file follow the pixels, so the colours
            // are read in their own space and the metadata can be kept.
            var icc = raster.Icc ?? [];
            var data = new byte[raster.Rgba.Length + icc.Length + bytes.Length];
            raster.Rgba.CopyTo(data, 0);
            icc.CopyTo(data, raster.Rgba.Length);
            bytes.CopyTo(data, raster.Rgba.Length + icc.Length);
            var args = new { width = raster.Width, height = raster.Height, depth = raster.Depth, space = raster.Space, iccLength = icc.Length };
            return (await engine.CallAsync("decodeRgba", args, data)).Json ?? new JsonObject();
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
        if (exception is FileNotFoundException or DirectoryNotFoundException) return I18n.T("文件不存在或已被移动");
        if (exception is UnauthorizedAccessException) return I18n.T("没有读取这个文件的权限");
        if (exception is IOException) return I18n.T("读取文件失败");
        if (message.StartsWith("RAW decode failed", StringComparison.Ordinal)) return I18n.T("无法解码这个 RAW 文件：相机型号可能还不支持，或文件已损坏");
        if (message.StartsWith("image decode failed", StringComparison.Ordinal) || message.Contains("unexpected pixel layout")) return I18n.T("无法识别这个文件的格式，或文件已损坏");
        if (message.Contains("no image loaded")) return I18n.T("还没有打开照片");
        // Messages the app wrote for people are already translated.
        if (exception is Engine.UserMessageException or FormatException) return message;
        return I18n.T("处理时出错");
    }

    /// <summary>The original text, for a tooltip, when it is not already the friendly one.</summary>
    public static string? Detail(Exception exception) => Friendly(exception) == exception.Message ? null : exception.Message;
}

/// <summary>A step in the history. Parent is the step it was made from; -1 for the opened state.</summary>
public sealed record HistoryEntry(string Label, Dictionary<string, double> Edit, List<LocalLayer> Locals, int Parent = -1);

/// <summary>How a step is listed: indented by how many branches it lies on, and whether it leads to where the photo is now.</summary>
public sealed record HistoryLine(int Index, string Label, int Depth, bool OnPath);

/// <summary>
/// One photo's edit history, including its local layers' shapes. Consecutive changes to
/// the same settings within a moment merge into one step, so a slider drag is undone in one go.
/// The history branches: editing after an undo starts a new branch and keeps the steps that
/// were undone, so any earlier version can be gone back to.
/// </summary>
public sealed class EditHistory
{
    private const int Limit = 200;
    /// <summary>For each step, the child redo goes to: the one made or visited last.</summary>
    private readonly Dictionary<int, int> _redo = [];
    private const long MergeMs = 1200;

    /// <summary>Keys the last unlabelled step changed; a quick step within them merges into it.</summary>
    private HashSet<string>? _lastKeys;
    /// <summary>The label of the last step that changed a layer's shape, for merging a drag.</summary>
    private string? _lastShape;
    private long _lastAt;

    private static HashSet<string> KeysOf(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after) =>
        Edits.ChangedKeys(before, after).Split(',', StringSplitOptions.RemoveEmptyEntries).ToHashSet();

    private static List<LocalLayer> Copy(IEnumerable<LocalLayer> layers) => layers.Select(layer => layer.Copy()).ToList();

    public EditHistory(IReadOnlyDictionary<string, double> opened, IEnumerable<LocalLayer>? locals = null)
    {
        Entries.Add(new(I18n.T("打开"), new(opened), Copy(locals ?? [])));
    }

    public List<HistoryEntry> Entries { get; } = [];
    public int Index { get; private set; }
    public int UndoTarget => Entries[Index].Parent;
    public int RedoTarget => _redo.TryGetValue(Index, out var child) ? child : -1;
    public bool CanUndo => UndoTarget >= 0;
    public bool CanRedo => RedoTarget >= 0;
    /// <summary>Whether a step has more than one way forward.</summary>
    public bool HasBranches => Entries.Skip(1).GroupBy(entry => entry.Parent).Any(children => children.Count() > 1);
    public Dictionary<string, double> Current => new(Entries[Index].Edit);
    public List<LocalLayer> CurrentLocals => Copy(Entries[Index].Locals);

    /// <summary>
    /// Records a change. label names an action; without one the change is described from its
    /// settings. shapeChanged says a local layer was added, moved or painted.
    /// </summary>
    public void Record(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after, string? label = null,
        IEnumerable<LocalLayer>? locals = null, bool shapeChanged = false)
    {
        var keys = KeysOf(before, after);
        if (keys.Count == 0 && !shapeChanged) return;
        var layers = Copy(locals ?? Entries[Index].Locals);
        var now = Environment.TickCount64;
        // Only the newest step, with nothing made from it, can absorb the next change.
        var recent = now - _lastAt < MergeMs && Index == Entries.Count - 1 && Index > 0 && !_redo.ContainsKey(Index);
        var parent = Entries[Index].Parent;
        if (shapeChanged && recent && label is not null && label == _lastShape)
        {
            Entries[Index] = new(label, new(after), layers, parent);
        }
        else if (!shapeChanged && label is null && _lastKeys is not null && keys.IsSubsetOf(_lastKeys) && recent)
        {
            var basis = Entries[parent];
            var merged = KeysOf(basis.Edit, after);
            if (merged.Count == 0)
            {
                // Dragged back to where it started: the step disappears.
                Entries.RemoveAt(Index);
                if (_redo.TryGetValue(parent, out var child) && child == Index) _redo.Remove(parent);
                Index = parent;
                _lastKeys = null;
                return;
            }
            Entries[Index] = new(Edits.DescribeChange(basis.Edit, after), new(after), layers, parent);
            _lastKeys.UnionWith(merged);
        }
        else
        {
            // A new step from wherever the history is; steps undone before stay as a branch.
            Entries.Add(new(label ?? Edits.DescribeChange(before, after), new(after), layers, Index));
            _redo[Index] = Entries.Count - 1;
            Index = Entries.Count - 1;
            if (Entries.Count > Limit) DropOldest();
            _lastKeys = label is null && !shapeChanged ? keys : null;
        }
        _lastShape = shapeChanged ? label : null;
        _lastAt = now;
    }

    public Dictionary<string, double> MoveTo(int index)
    {
        Index = Math.Clamp(index, 0, Entries.Count - 1);
        // Redo from any step on the way back retraces the path to here.
        for (var at = Index; Entries[at].Parent >= 0; at = Entries[at].Parent) _redo[Entries[at].Parent] = at;
        _lastKeys = null;
        _lastShape = null;
        return Current;
    }

    /// <summary>Forgets the oldest step after the opened state; what was made from it now comes from the opened state.</summary>
    private void DropOldest()
    {
        const int dropped = 1;
        Entries.RemoveAt(dropped);
        int Shift(int index) => index == dropped ? 0 : index > dropped ? index - 1 : index;
        for (var i = 1; i < Entries.Count; i++) Entries[i] = Entries[i] with { Parent = Shift(Entries[i].Parent) };
        var redo = _redo.Where(pair => pair.Key != dropped && pair.Value != dropped).ToDictionary(pair => Shift(pair.Key), pair => Shift(pair.Value));
        _redo.Clear();
        foreach (var (from, to) in redo) _redo[from] = to;
        Index = Shift(Index);
    }

    /// <summary>
    /// The steps in reading order: each step, then what was made from it. The first line
    /// forward continues at the same depth; later branches are indented below the step they left.
    /// </summary>
    public List<HistoryLine> Lines()
    {
        var children = Entries.Select((_, index) => new List<int>()).ToList();
        for (var i = 1; i < Entries.Count; i++) children[Entries[i].Parent].Add(i);
        var path = new HashSet<int>();
        for (var at = Index; at >= 0; at = Entries[at].Parent) path.Add(at);
        var lines = new List<HistoryLine>();
        var stack = new Stack<(int Index, int Depth)>();
        stack.Push((0, 0));
        while (stack.Count > 0)
        {
            var (at, depth) = stack.Pop();
            lines.Add(new(at, Entries[at].Label, depth, path.Contains(at)));
            var next = children[at];
            // Pushed in reverse so the first child is listed first: the main line, then the branches.
            for (var c = next.Count - 1; c >= 0; c--) stack.Push((next[c], depth + (c == 0 ? 0 : 1)));
        }
        return lines;
    }
}
