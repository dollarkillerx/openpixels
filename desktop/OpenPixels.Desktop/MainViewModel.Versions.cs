using CommunityToolkit.Mvvm.Input;

namespace OpenPixels.Desktop;

/// <summary>Which versions each photo has beyond itself, kept by the photo's own id.</summary>
public sealed class VersionStore
{
    private readonly JsonFile<Dictionary<string, List<int>>> _file = new("versions.json");
    private readonly Dictionary<string, List<int>> _versions;

    public VersionStore() => _versions = _file.Load();

    /// <summary>The version numbers made from a photo, oldest first; never 1, which is the photo itself.</summary>
    public IReadOnlyList<int> Of(string sourceId) => _versions.TryGetValue(sourceId, out var versions) ? versions : [];

    public int Add(string sourceId)
    {
        if (!_versions.TryGetValue(sourceId, out var versions)) _versions[sourceId] = versions = [];
        var next = versions.Count == 0 ? 2 : versions.Max() + 1;
        versions.Add(next);
        _file.Save(_versions);
        return next;
    }

    public void Remove(string sourceId, int version)
    {
        if (!_versions.TryGetValue(sourceId, out var versions)) return;
        versions.Remove(version);
        if (versions.Count == 0) _versions.Remove(sourceId);
        _file.Save(_versions);
    }
}

/// <summary>
/// Versions, like Lightroom's virtual copies: one file edited several ways, each with its own
/// settings, local layers and retouching, listed beside the photo and exported on its own.
/// </summary>
public sealed partial class MainViewModel
{
    private readonly VersionStore _versions = new();

    private PhotoItem Culled(PhotoItem photo)
    {
        photo.Rating = _cull.Rating(photo.Id);
        photo.Flag = _cull.Flag(photo.Id);
        photo.Label = _cull.Label(photo.Id);
        return photo;
    }

    public bool CanDeleteVersion => SelectedPhoto?.IsVersion == true;

    /// <summary>Makes a version of the open photo that starts from its current edit, and opens it.</summary>
    [RelayCommand(CanExecute = nameof(CanEdit))]
    public void CreateVersion()
    {
        if (SelectedPhoto is not { } source || !_imageOpen) return;
        var number = _versions.Add(source.SourceId);
        var version = new PhotoItem(source.Path, number);
        // Everything that makes up the edit comes along; ratings and flags start fresh.
        _store.Save(version.Id, _edit);
        _retouch.SaveLocals(version.Id, _locals);
        _retouch.SaveHeals(version.Id, (System.Text.Json.Nodes.JsonArray)_heals.DeepClone());
        _retouch.SaveErases(version.Id, _erases);
        _retouch.SaveWarps(version.Id, _warps);
        var after = Photos.Select((photo, index) => (photo, index)).Where(entry => entry.photo.Path == source.Path).Max(entry => entry.index);
        Photos.Insert(after + 1, Culled(version));
        version.Thumbnail = source.Thumbnail;
        RefreshVisible();
        SelectedPhotos.Clear();
        SelectedPhoto = version;
        SelectedPhotos.Add(version);
        Status = I18n.T("已新建{name}", ("name", version.Name));
    }

    /// <summary>Removes the open version and its edit; the photo itself is never removed this way.</summary>
    [RelayCommand]
    public void DeleteVersion()
    {
        if (SelectedPhoto is not { IsVersion: true } version) return;
        var source = Photos.FirstOrDefault(photo => photo.Path == version.Path && !photo.IsVersion);
        _versions.Remove(version.SourceId, version.Version);
        _store.Save(version.Id, new Dictionary<string, double>());
        _retouch.SaveLocals(version.Id, []);
        _retouch.SaveHeals(version.Id, []);
        _retouch.SaveErases(version.Id, []);
        _retouch.SaveWarps(version.Id, []);
        Photos.Remove(version);
        RefreshVisible();
        SelectedPhotos.Clear();
        SelectedPhoto = source ?? Photos.FirstOrDefault();
        if (SelectedPhoto is { } open) SelectedPhotos.Add(open);
        Status = I18n.T("已删除{name}", ("name", version.Name));
    }
}
