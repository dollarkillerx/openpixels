using System.Collections.ObjectModel;
using System.Text.Json.Nodes;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

/// <summary>Culling, presets and syncing, batch export, soft proofing and projects.</summary>
public sealed partial class MainViewModel
{
    private const int AnalysisLongEdge = 1024;

    private Dictionary<string, PhotoAnalysis>? _analyses;
    private readonly Dictionary<string, Verdict> _verdicts = [];
    private bool _filtering;
    private bool _exportCancelled;
    private PhotoItem? _selectedPhoto;

    /// <summary>The photo open in the editor. The list may briefly drop it while filtering; that is ignored.</summary>
    public PhotoItem? SelectedPhoto
    {
        get => _selectedPhoto;
        set
        {
            if (value is null && _filtering)
            {
                OnPropertyChanged();
                return;
            }
            if (SetProperty(ref _selectedPhoto, value) && value is not null) _ = OpenAsync(value);
        }
    }

    /// <summary>The photos that pass the filters, in import order.</summary>
    public ObservableCollection<PhotoItem> VisiblePhotos { get; } = [];

    /// <summary>Photos selected in the filmstrip or grid; the open photo is one of them.</summary>
    public ObservableCollection<PhotoItem> SelectedPhotos { get; } = [];

    public string[] RatingFilters { get; } = ["全部星级", "★ 以上", "★★ 以上", "★★★ 以上", "★★★★ 以上", "★★★★★"];
    public string[] FlagFilters { get; } = ["全部旗标", "已选中", "未淘汰", "已淘汰"];
    public string[] LabelFilters { get; } = ["全部色标", .. ColourLabels.All.Select(label => label.Name)];
    public ObservableCollection<string> GroupFilters { get; } = ["全部分组"];

    [ObservableProperty]
    private int _ratingFilterIndex;

    [ObservableProperty]
    private int _flagFilterIndex;

    [ObservableProperty]
    private int _labelFilterIndex;

    [ObservableProperty]
    private int _groupFilterIndex;

    [ObservableProperty]
    private bool _isGridView;

    [ObservableProperty]
    private bool _isBusy;

    [ObservableProperty]
    private string _libraryStatus = "";

    partial void OnRatingFilterIndexChanged(int value) => RefreshVisible();
    partial void OnFlagFilterIndexChanged(int value) => RefreshVisible();
    partial void OnLabelFilterIndexChanged(int value) => RefreshVisible();
    partial void OnGroupFilterIndexChanged(int value) => RefreshVisible();

    public string VisibleCount => VisiblePhotos.Count == Photos.Count ? $"{Photos.Count} 张" : $"{VisiblePhotos.Count} / {Photos.Count} 张";

    private bool Passes(PhotoItem photo)
    {
        if (photo.Rating < RatingFilterIndex) return false;
        var flagOk = FlagFilterIndex switch
        {
            1 => photo.Flag == CullStore.FlagPick,
            2 => photo.Flag != CullStore.FlagReject,
            3 => photo.Flag == CullStore.FlagReject,
            _ => true,
        };
        if (!flagOk) return false;
        if (LabelFilterIndex > 0 && photo.Label != LabelFilterIndex) return false;
        return GroupFilterIndex switch
        {
            <= 0 => true,
            // Photos not analysed yet count as their own best.
            1 => !_verdicts.TryGetValue(photo.Id, out var verdict) || verdict.Best,
            var index => photo.Group == index - 1,
        };
    }

    /// <summary>Brings the visible list in line with the filters, moving as few items as possible.</summary>
    private void RefreshVisible()
    {
        _filtering = true;
        try
        {
            var wanted = Photos.Where(Passes).ToList();
            for (var i = VisiblePhotos.Count - 1; i >= 0; i--)
            {
                if (!wanted.Contains(VisiblePhotos[i])) VisiblePhotos.RemoveAt(i);
            }
            for (var i = 0; i < wanted.Count; i++)
            {
                if (i < VisiblePhotos.Count && VisiblePhotos[i] == wanted[i]) continue;
                VisiblePhotos.Insert(i, wanted[i]);
            }
        }
        finally
        {
            _filtering = false;
        }
        // Removing and re-adding items drops them from the list's selection; keep the open photo selected.
        if (SelectedPhoto is { } open && VisiblePhotos.Contains(open) && !SelectedPhotos.Contains(open)) SelectedPhotos.Add(open);
        OnPropertyChanged(nameof(VisibleCount));
    }

    // ---- Stars, flags and labels ----

    /// <summary>What a rating, flag or label applies to: the selection, or the open photo.</summary>
    private List<PhotoItem> Targets() =>
        SelectedPhotos.Count > 1 ? [.. SelectedPhotos] : SelectedPhoto is { } photo ? [photo] : [];

    public void SetRating(int rating)
    {
        foreach (var photo in Targets())
        {
            photo.Rating = rating;
            _cull.SetRating(photo.Id, rating);
        }
        RefreshVisible();
    }

    public void SetFlag(int flag)
    {
        foreach (var photo in Targets())
        {
            photo.Flag = flag;
            _cull.SetFlag(photo.Id, flag);
        }
        RefreshVisible();
    }

    /// <summary>Sets a colour label, or clears it if every target already has it.</summary>
    public void ToggleLabel(int label)
    {
        var targets = Targets();
        var value = targets.All(photo => photo.Label == label) ? 0 : label;
        foreach (var photo in targets)
        {
            photo.Label = value;
            _cull.SetLabel(photo.Id, value);
        }
        RefreshVisible();
    }

    [RelayCommand]
    private void Rate(string rating) => SetRating(int.Parse(rating));

    [RelayCommand]
    private void Pick() => SetFlag(CullStore.FlagPick);

    [RelayCommand]
    private void Reject() => SetFlag(CullStore.FlagReject);

    [RelayCommand]
    private void Unflag() => SetFlag(0);

    [RelayCommand]
    private void Labelled(string label) => ToggleLabel(int.Parse(label));

    [RelayCommand]
    public void ToggleGrid() => IsGridView = !IsGridView;

    // ---- Smart grouping ----

    private Dictionary<string, PhotoAnalysis> Analyses => _analyses ??= _analysisFile.Load();

    /// <summary>Runs one job on the background engine, after any job already using it.</summary>
    private async Task<T> WithBackgroundEngine<T>(Func<EngineClient, Task<T>> job)
    {
        await _backgroundTurn.WaitAsync();
        try
        {
            var engine = _thumbnailer.Value;
            await engine.Ready;
            return await job(engine);
        }
        finally
        {
            _backgroundTurn.Release();
        }
    }

    /// <summary>Opens a photo with its saved edit on a background engine. Returns what decode reports.</summary>
    private async Task<JsonNode> LoadForBackgroundAsync(EngineClient engine, PhotoItem photo, int proxyLongEdge)
    {
        var info = await PhotoLoader.DecodeAsync(engine, photo.Path);
        await engine.CallAsync("buildProxy", new { maxLongEdge = proxyLongEdge });
        await PhotoLoader.ApplyLensDatabaseAsync(engine, info);
        var edit = photo == SelectedPhoto && _imageOpen ? _edit : _store.Load(photo.Id);
        await engine.CallRawAsync("setAdjustments", Edits.ToEngineJson(edit));
        await SetLookAsync(engine, (int)edit.GetValueOrDefault("lutId"));
        return info;
    }

    /// <summary>Measures every photo not yet analysed, then groups them all.</summary>
    [RelayCommand]
    public async Task Analyze()
    {
        var pending = Photos.Where(photo => !Analyses.ContainsKey(photo.Id)).ToList();
        if (pending.Count == 0)
        {
            ApplyGroups();
            LibraryStatus = Photos.Count == 0 ? "" : $"{Photos.Count} 张都已分析过，共 {GroupFilters.Count - 2} 组。";
            return;
        }
        IsBusy = true;
        var failed = 0;
        try
        {
            for (var index = 0; index < pending.Count; index++)
            {
                var photo = pending[index];
                LibraryStatus = $"正在分析 {index + 1} / {pending.Count}…";
                try
                {
                    Analyses[photo.Id] = await WithBackgroundEngine(async engine =>
                    {
                        var info = await LoadForBackgroundAsync(engine, photo, AnalysisLongEdge);
                        var size = (await engine.CallAsync("thumbnailSize", new { maxLongEdge = AnalysisLongEdge })).Json!;
                        var rgb = (await engine.CallAsync("thumbnail", new { maxLongEdge = AnalysisLongEdge })).Data;
                        var captured = Analysis.CaptureMillis(info["captureTime"]?.GetValue<string>());
                        var fileTime = (File.GetLastWriteTimeUtc(photo.Path) - DateTime.UnixEpoch).TotalMilliseconds;
                        return await Task.Run(() => Analysis.Analyze(rgb, (int)size["width"]!, (int)size["height"]!, captured ?? fileTime, captured is not null));
                    });
                }
                catch (Exception)
                {
                    failed++;
                }
            }
            _analysisFile.Save(Analyses);
        }
        finally
        {
            IsBusy = false;
        }
        ApplyGroups();
        LibraryStatus = $"已分析 {pending.Count - failed} 张，共 {GroupFilters.Count - 2} 组。" + (failed > 0 ? $"{failed} 张无法读取。" : "");
    }

    /// <summary>Numbers the groups among the photos that have been analysed, and judges each group.</summary>
    private void ApplyGroups()
    {
        var items = Photos.Where(photo => Analyses.ContainsKey(photo.Id)).Select(photo => (photo.Id, Analyses[photo.Id])).ToList();
        if (items.Count == 0) return;
        var groups = Analysis.Group(items);
        var byId = items.ToDictionary(item => item.Id, item => item.Item2);
        _verdicts.Clear();
        var groupOf = new Dictionary<string, int>();
        for (var index = 0; index < groups.Count; index++)
        {
            foreach (var (id, verdict) in Analysis.Judge(groups[index].Select(id => (id, byId[id])).ToList()))
            {
                _verdicts[id] = verdict;
                groupOf[id] = index + 1;
            }
        }
        foreach (var photo in Photos)
        {
            photo.Group = groupOf.GetValueOrDefault(photo.Id);
            photo.IsBest = _verdicts.TryGetValue(photo.Id, out var verdict) && verdict.Best && groups.Count < items.Count;
        }
        // Updated in place: clearing the list would reset the filter's selection and empty the filmstrip.
        string[] names = ["全部分组", "每组最佳", .. groups.Select((group, index) => $"组 {index + 1}（{group.Count} 张）")];
        for (var index = 0; index < names.Length; index++)
        {
            if (index < GroupFilters.Count && GroupFilters[index] != names[index]) GroupFilters[index] = names[index];
            else if (index >= GroupFilters.Count) GroupFilters.Add(names[index]);
        }
        while (GroupFilters.Count > names.Length) GroupFilters.RemoveAt(GroupFilters.Count - 1);
        if (GroupFilterIndex >= GroupFilters.Count || GroupFilterIndex < 0) GroupFilterIndex = 0;
        RefreshVisible();
    }

    /// <summary>Flags the best and the clearly failed shots of each group, keeping manual flags.</summary>
    [RelayCommand]
    public void AutoPick()
    {
        var (picked, rejected) = (0, 0);
        foreach (var photo in Photos)
        {
            if (!_verdicts.TryGetValue(photo.Id, out var verdict) || photo.Flag != 0) continue;
            if (verdict.Best)
            {
                photo.Flag = CullStore.FlagPick;
                picked++;
            }
            else if (verdict.Reject)
            {
                photo.Flag = CullStore.FlagReject;
                rejected++;
            }
            else
            {
                continue;
            }
            _cull.SetFlag(photo.Id, photo.Flag);
        }
        RefreshVisible();
        LibraryStatus = _verdicts.Count == 0 ? "先点“智能分组”分析照片。" : $"选中 {picked} 张，淘汰 {rejected} 张。已手动标记的照片没有改动。";
    }

    // ---- Presets, copy and paste, sync ----

    public ObservableCollection<string> PresetNames { get; } = [];

    [ObservableProperty]
    private string? _selectedPreset;

    [ObservableProperty]
    private string _newPresetName = "";

    private Dictionary<string, double>? _copied;

    [ObservableProperty]
    private bool _hasCopied;

    public int OthersSelected => SelectedPhotos.Count(photo => photo != SelectedPhoto);
    public string PasteLabel => OthersSelected > 0 ? $"粘贴到 {OthersSelected + 1} 张" : "粘贴设置";
    public string SyncSelectedLabel => $"同步到选中（{OthersSelected}）";
    public bool HasOthersSelected => CanEdit && OthersSelected > 0;

    private void RefreshPresetNames()
    {
        PresetNames.Clear();
        foreach (var name in _presets.All.Keys.Order()) PresetNames.Add(name);
    }

    [RelayCommand]
    private void SavePreset()
    {
        var name = NewPresetName.Trim();
        if (name.Length == 0 || !_imageOpen) return;
        _presets.Save(name, _edit);
        RefreshPresetNames();
        SelectedPreset = name;
        NewPresetName = "";
        Status = $"已保存预设 {name}";
    }

    [RelayCommand]
    private void ApplyPreset()
    {
        if (SelectedPreset is { } name && _presets.All.TryGetValue(name, out var preset)) ApplyEdit(Edits.Merge(preset, _edit), $"预设 {name}");
    }

    [RelayCommand]
    private void DeletePreset()
    {
        if (SelectedPreset is not { } name) return;
        _presets.Delete(name);
        RefreshPresetNames();
        SelectedPreset = null;
    }

    [RelayCommand]
    private void CopySettings()
    {
        if (!_imageOpen) return;
        _copied = EditStore.Kept(_edit);
        HasCopied = true;
        Status = "已复制调整设置";
    }

    /// <summary>Pastes onto the open photo and every other selected one.</summary>
    [RelayCommand]
    private void PasteSettings()
    {
        if (_copied is null) return;
        ApplyEdit(Edits.Merge(_copied, _edit), "粘贴设置");
        ApplySettingsTo(SelectedPhotos.Where(photo => photo != SelectedPhoto).ToList(), _copied, "粘贴设置");
    }

    [RelayCommand]
    private void SyncToSelected() => ApplySettingsTo(SelectedPhotos.Where(photo => photo != SelectedPhoto).ToList(), EditStore.Kept(_edit), "同步设置");

    [RelayCommand]
    private void SyncToAll() => ApplySettingsTo(Photos.Where(photo => photo != SelectedPhoto).ToList(), EditStore.Kept(_edit), "同步设置");

    /// <summary>Gives other photos source's settings, keeping their framing; each can be undone when opened.</summary>
    private void ApplySettingsTo(IReadOnlyList<PhotoItem> targets, IReadOnlyDictionary<string, double> source, string label)
    {
        if (!_imageOpen || targets.Count == 0) return;
        foreach (var photo in targets)
        {
            var previous = _store.Load(photo.Id);
            var merged = Edits.Merge(source, previous);
            if (!_histories.TryGetValue(photo.Id, out var history)) _histories[photo.Id] = history = new EditHistory(previous);
            history.Record(history.Current, merged, label);
            _store.Save(photo.Id, merged);
            photo.ThumbnailStale = true;
        }
        Status = $"已把设置{(label == "粘贴设置" ? "粘贴" : "同步")}到 {targets.Count} 张照片";
        _ = FillThumbnailsAsync();
    }

    private void OnSelectionChanged()
    {
        OnPropertyChanged(nameof(OthersSelected));
        OnPropertyChanged(nameof(PasteLabel));
        OnPropertyChanged(nameof(SyncSelectedLabel));
        OnPropertyChanged(nameof(HasOthersSelected));
    }

    // ---- Batch export ----

    /// <summary>Exports photos one after another on the background engine, so editing goes on meanwhile.</summary>
    public async Task<int> ExportManyAsync(IReadOnlyList<PhotoItem> photos, OutputSettings settings, string folder)
    {
        SaveOutput(settings);
        _exportCancelled = false;
        IsBusy = true;
        var used = Directory.EnumerateFiles(folder).Select(file => Path.GetFileName(file).ToLowerInvariant()).ToHashSet();
        var (done, failed) = (0, new List<string>());
        var started = Environment.TickCount64;
        try
        {
            for (var index = 0; index < photos.Count && !_exportCancelled; index++)
            {
                var photo = photos[index];
                LibraryStatus = $"正在导出 {index + 1} / {photos.Count}：{photo.Name}";
                try
                {
                    var bytes = await WithBackgroundEngine(async engine =>
                    {
                        await LoadForBackgroundAsync(engine, photo, ThumbnailProxyLongEdge);
                        if ((await engine.CallAsync("hasFullImage")).Json?.GetValue<bool>() == false) await engine.CallAsync("developFull");
                        var size = (await engine.CallAsync("exportSize", new { longEdge = settings.LongEdge })).Json!;
                        var overlay = global::OpenPixels.Desktop.Output.RenderWatermark(settings.Watermark, (int)size["width"]!, (int)size["height"]!);
                        var args = new
                        {
                            settings = settings.EngineSettings,
                            overlayX = overlay?.X ?? 0,
                            overlayY = overlay?.Y ?? 0,
                            overlayWidth = overlay?.Width ?? 0,
                            overlayHeight = overlay?.Height ?? 0,
                        };
                        return (await engine.CallAsync("exportImage", args, overlay?.Rgba)).Data;
                    });
                    var name = global::OpenPixels.Desktop.Output.ExportName(photo.Name, used, settings.Template, index + 1, settings.Extension);
                    await File.WriteAllBytesAsync(Path.Combine(folder, name), bytes);
                    done++;
                }
                catch (Exception)
                {
                    failed.Add(photo.Name);
                }
            }
        }
        finally
        {
            IsBusy = false;
        }
        var seconds = (Environment.TickCount64 - started) / 1000.0;
        LibraryStatus = (_exportCancelled ? "已取消。" : "") + $"已导出 {done} 张（{seconds:0.0} 秒）"
            + (failed.Count > 0 ? $"，{failed.Count} 张失败：{string.Join("、", failed.Take(3))}{(failed.Count > 3 ? " 等" : "")}" : "");
        Status = LibraryStatus;
        return done;
    }

    [RelayCommand]
    private void CancelBatch() => _exportCancelled = true;

    // ---- Soft proofing ----

    public ObservableCollection<ProofProfile> ProofProfiles { get; } = [];
    public string[] ProofIntents { get; } = ["可感知", "相对比色"];

    [ObservableProperty]
    private ProofProfile? _selectedProofProfile;

    [ObservableProperty]
    private bool _proofOn;

    [ObservableProperty]
    private int _proofIntentIndex;

    [ObservableProperty]
    private bool _proofPaper;

    [ObservableProperty]
    private bool _proofGamut;

    [ObservableProperty]
    private string _proofStatus = "";

    private bool _loadingProof;

    public bool PaperAvailable => SelectedProofProfile?.IsOutput == true;
    public string ProofBadge => ProofOn && SelectedProofProfile is { } profile ? $"软打样：{profile.Name}" : "";

    partial void OnProofOnChanged(bool value) => ProofChanged();
    partial void OnSelectedProofProfileChanged(ProofProfile? value) => ProofChanged();
    partial void OnProofIntentIndexChanged(int value) => ProofChanged();
    partial void OnProofPaperChanged(bool value) => ProofChanged();
    partial void OnProofGamutChanged(bool value) => ProofChanged();

    private void ProofChanged()
    {
        OnPropertyChanged(nameof(PaperAvailable));
        OnPropertyChanged(nameof(ProofBadge));
        if (_loadingProof) return;
        _proofFile.Save(new ProofSettings
        {
            On = ProofOn, ProfilePath = SelectedProofProfile?.Path, Perceptual = ProofIntentIndex == 0, Paper = ProofPaper, Gamut = ProofGamut,
        });
        _ = Fire(SendProofAsync);
    }

    /// <summary>Lists the system's and the imported ICC profiles the engine can proof with.</summary>
    private async Task LoadProfilesAsync()
    {
        var files = await Task.Run(() => ProfileFolders.Files().Distinct().ToList());
        var found = new List<ProofProfile>();
        foreach (var file in files)
        {
            try
            {
                if (new FileInfo(file).Length > 8 << 20) continue;
                var info = (await _engine.CallAsync("describeProfile", null, await File.ReadAllBytesAsync(file))).Json!;
                found.Add(new ProofProfile(file, info["name"]?.GetValue<string>() ?? Path.GetFileNameWithoutExtension(file), info["space"]!.GetValue<string>(), info["isOutput"]?.GetValue<bool>() == true));
            }
            catch (Exception exception) when (exception is EngineException or IOException or UnauthorizedAccessException)
            {
                // Abstract and device-link profiles, among others, cannot be proofed with.
            }
        }
        // Several display profiles can share one name; the file name tells them apart.
        var shared = found.GroupBy(profile => profile.Name).Where(group => group.Count() > 1).Select(group => group.Key).ToHashSet();
        found = found.Select(profile => shared.Contains(profile.Name) ? profile with { Name = $"{profile.Name} · {Path.GetFileNameWithoutExtension(profile.Path)}" } : profile).ToList();
        _loadingProof = true;
        ProofProfiles.Clear();
        foreach (var profile in found.OrderBy(profile => profile.Space != "CMYK").ThenBy(profile => profile.Name, StringComparer.CurrentCulture)) ProofProfiles.Add(profile);
        var saved = _proofFile.Load();
        SelectedProofProfile = ProofProfiles.FirstOrDefault(profile => profile.Path == saved.ProfilePath);
        ProofIntentIndex = saved.Perceptual ? 0 : 1;
        ProofPaper = saved.Paper;
        ProofGamut = saved.Gamut;
        ProofOn = saved.On && SelectedProofProfile is not null;
        _loadingProof = false;
        OnPropertyChanged(nameof(PaperAvailable));
        OnPropertyChanged(nameof(ProofBadge));
        ProofStatus = $"找到 {ProofProfiles.Count} 个可用的 ICC 配置。";
        if (ProofOn) await SendProofAsync();
    }

    /// <summary>Copies an ICC file into the data folder and selects it.</summary>
    public async Task ImportProfileAsync(string path)
    {
        try
        {
            var info = (await _engine.CallAsync("describeProfile", null, await File.ReadAllBytesAsync(path))).Json!;
            Directory.CreateDirectory(ProfileFolders.Imported);
            var copy = Path.Combine(ProfileFolders.Imported, Path.GetFileName(path));
            File.Copy(path, copy, overwrite: true);
            var profile = new ProofProfile(copy, info["name"]?.GetValue<string>() ?? Path.GetFileNameWithoutExtension(path), info["space"]!.GetValue<string>(), info["isOutput"]?.GetValue<bool>() == true);
            ProofProfiles.Insert(0, profile);
            SelectedProofProfile = profile;
            ProofOn = true;
        }
        catch (Exception exception)
        {
            Status = $"无法使用这个 ICC 配置：{Messages.Friendly(exception)}";
            StatusDetail = Messages.Detail(exception);
        }
    }

    private async Task SendProofAsync()
    {
        if (ProofOn && SelectedProofProfile is { } profile)
        {
            var bytes = await File.ReadAllBytesAsync(profile.Path);
            await _engine.CallAsync("setProof", new { perceptual = ProofIntentIndex == 0, paper = ProofPaper && profile.IsOutput, gamut = ProofGamut }, bytes);
        }
        else
        {
            await _engine.CallAsync("clearProof");
        }
        RequestRender();
        // The histogram follows the proof.
        _ = RefreshHistogramAsync();
    }

    [RelayCommand]
    public void ToggleProof()
    {
        if (SelectedProofProfile is null)
        {
            Status = "先在“软打样”里选择一个 ICC 配置";
            return;
        }
        ProofOn = !ProofOn;
    }

    // ---- Projects ----

    public ObservableCollection<Project> Projects { get; } = [];

    [ObservableProperty]
    private string _newProjectName = "";

    public Project? LastSession => _projects.Last is { Paths.Count: > 0 } last ? last : null;
    public bool CanResume => Photos.Count == 0 && LastSession is not null;
    public string ResumeLabel => LastSession is { } last ? $"继续上次的 {last.Paths.Count} 张照片" : "";

    private void RefreshProjects()
    {
        Projects.Clear();
        foreach (var project in _projects.Named) Projects.Add(project);
    }

    /// <summary>Remembers the photos open now, to offer them again next time.</summary>
    private void SaveSession()
    {
        if (Photos.Count > 0) _projects.Save(ProjectStore.LastSession, Photos.Select(photo => photo.Path));
    }

    [RelayCommand]
    private void SaveProject()
    {
        var name = NewProjectName.Trim();
        if (name.Length == 0 || Photos.Count == 0) return;
        _projects.Save(name, Photos.Select(photo => photo.Path));
        NewProjectName = "";
        RefreshProjects();
        Status = $"已保存项目 {name}（{Photos.Count} 张）";
    }

    [RelayCommand]
    private void OpenProject(Project project) => ReplacePhotos(project.Paths, project.Name);

    [RelayCommand]
    private void DeleteProject(Project project)
    {
        _projects.Delete(project.Name);
        RefreshProjects();
    }

    [RelayCommand]
    private void Resume()
    {
        if (LastSession is { } last) ReplacePhotos(last.Paths, "上次的照片");
    }

    /// <summary>Closes the photos open now and opens these instead.</summary>
    private void ReplacePhotos(IReadOnlyList<string> paths, string name)
    {
        var existing = paths.Where(File.Exists).ToList();
        _openTicket++;
        _imageOpen = false;
        CanEdit = false;
        Preview = null;
        Histogram = null;
        SelectedPhotos.Clear();
        _selectedPhoto = null;
        OnPropertyChanged(nameof(SelectedPhoto));
        Photos.Clear();
        RefreshVisible();
        AddPhotos(existing);
        var missing = paths.Count - existing.Count;
        Status = $"已打开 {name}" + (missing > 0 ? $"，{missing} 张照片已找不到" : "");
    }
}
