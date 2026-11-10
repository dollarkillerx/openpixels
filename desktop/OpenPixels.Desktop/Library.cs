namespace OpenPixels.Desktop;

/// <summary>Stars, flags and colour labels, kept per photo like edits.</summary>
public sealed class CullStore
{
    public const int FlagPick = 1;
    public const int FlagReject = -1;

    private readonly JsonFile<Dictionary<string, int>> _ratingFile = new("ratings.json");
    private readonly JsonFile<Dictionary<string, int>> _flagFile = new("flags.json");
    private readonly JsonFile<Dictionary<string, int>> _labelFile = new("labels.json");
    private readonly Dictionary<string, int> _ratings;
    private readonly Dictionary<string, int> _flags;
    private readonly Dictionary<string, int> _labels;

    public CullStore()
    {
        _ratings = _ratingFile.Load();
        _flags = _flagFile.Load();
        _labels = _labelFile.Load();
    }

    public int Rating(string id) => _ratings.GetValueOrDefault(id);
    public int Flag(string id) => _flags.GetValueOrDefault(id);
    public int Label(string id) => _labels.GetValueOrDefault(id);

    private static void Set(JsonFile<Dictionary<string, int>> file, Dictionary<string, int> all, string id, int value)
    {
        if (value != 0) all[id] = value;
        else all.Remove(id);
        file.Save(all);
    }

    public void SetRating(string id, int rating) => Set(_ratingFile, _ratings, id, Math.Clamp(rating, 0, 5));
    public void SetFlag(string id, int flag) => Set(_flagFile, _flags, id, Math.Clamp(flag, -1, 1));
    public void SetLabel(string id, int label) => Set(_labelFile, _labels, id, label);
}

/// <summary>Colour labels, as in the web edition: the stored value is the index plus one.</summary>
public static class ColourLabels
{
    public static readonly (string Name, string Colour)[] All =
        [(I18n.T("红"), "#e0483e"), (I18n.T("黄"), "#e5cb3a"), (I18n.T("绿"), "#55b04a"), (I18n.T("蓝"), "#3f73d8"), (I18n.T("紫"), "#7e52c8")];
}

/// <summary>Named edits that can be applied to any photo.</summary>
public sealed class PresetStore
{
    private readonly JsonFile<Dictionary<string, Dictionary<string, double>>> _file = new("presets.json");

    public PresetStore()
    {
        All = _file.Load();
    }

    public Dictionary<string, Dictionary<string, double>> All { get; }

    public void Save(string name, IReadOnlyDictionary<string, double> edit)
    {
        All[name] = EditStore.Kept(edit).Where(pair => !Edits.BelongsToOnePhoto(pair.Key)).ToDictionary();
        _file.Save(All);
    }

    public void Delete(string name)
    {
        All.Remove(name);
        _file.Save(All);
    }
}

public sealed record Project(string Name, List<string> Paths, DateTime Saved)
{
    [System.Text.Json.Serialization.JsonIgnore]
    public string CountLabel => I18n.T("{count} 张", ("count", Paths.Count));
}

/// <summary>Named sets of photos, and the set open when the app last closed.</summary>
public sealed class ProjectStore
{
    /// <summary>The name the last session is kept under; never shown as a project.</summary>
    public const string LastSession = "\u0000last";

    private readonly JsonFile<Dictionary<string, Project>> _file = new("projects.json");
    private readonly Dictionary<string, Project> _projects;

    public ProjectStore()
    {
        _projects = _file.Load();
    }

    public IEnumerable<Project> Named => _projects.Where(pair => pair.Key != LastSession).Select(pair => pair.Value).OrderByDescending(project => project.Saved);

    public Project? Last => _projects.GetValueOrDefault(LastSession);

    public void Save(string name, IEnumerable<string> paths)
    {
        _projects[name] = new Project(name == LastSession ? I18n.T("上次的照片") : name, paths.ToList(), DateTime.Now);
        _file.Save(_projects);
    }

    public void Delete(string name)
    {
        _projects.Remove(name);
        _file.Save(_projects);
    }
}

/// <summary>How the preview simulates an ICC profile. A viewing option, not part of any edit.</summary>
public sealed class ProofSettings
{
    public bool On { get; set; }
    public string? ProfilePath { get; set; }
    public bool Perceptual { get; set; } = true;
    /// <summary>Shows the paper's white and the ink's black instead of the screen's.</summary>
    public bool Paper { get; set; }
    /// <summary>Marks colours the profile cannot reproduce.</summary>
    public bool Gamut { get; set; }
}

public sealed record ProofProfile(string Path, string Name, string Space, bool IsOutput)
{
    public override string ToString() => $"{Name}（{Space}）";
}

/// <summary>Where the operating system keeps ICC profiles, plus the app's own imported ones.</summary>
public static class ProfileFolders
{
    public static string Imported => Path.Combine(DataFolder.Path, "profiles");

    public static IEnumerable<string> All()
    {
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (OperatingSystem.IsMacOS())
        {
            yield return "/System/Library/ColorSync/Profiles";
            yield return "/Library/ColorSync/Profiles";
            yield return Path.Combine(home, "Library/ColorSync/Profiles");
        }
        else if (OperatingSystem.IsWindows())
        {
            yield return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "spool", "drivers", "color");
        }
        else
        {
            yield return "/usr/share/color/icc";
            yield return Path.Combine(home, ".local/share/icc");
        }
        yield return Imported;
    }

    public static IEnumerable<string> Files() =>
        All().Where(Directory.Exists).SelectMany(folder =>
        {
            try
            {
                return Directory.EnumerateFiles(folder, "*.ic*", SearchOption.AllDirectories)
                    .Where(file => Path.GetExtension(file).ToLowerInvariant() is ".icc" or ".icm").ToList();
            }
            catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
            {
                return [];
            }
        });
}
