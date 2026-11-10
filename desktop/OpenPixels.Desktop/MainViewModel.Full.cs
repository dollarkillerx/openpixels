using System.Collections.ObjectModel;
using System.Text.Json.Nodes;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

/// <summary>The full edition's tools: liquify, background and sky replacement, and colour match.</summary>
public sealed partial class MainViewModel
{
    /// <summary>Pointer travel, in device pixels, below which a liquify drag waits for more.</summary>
    private const double MinLiquifyStep = 2;
    /// <summary>The sky's blend into the foreground when a sky is first chosen, as in the web edition.</summary>
    private const double DefaultRelight = 0.3;
    private const int MatchLongEdge = 256;

    private readonly ReplacementLibrary _replacements = new();
    /// <summary>Model masks before refinement, per photo; the engine refines these itself for replacement.</summary>
    private readonly Dictionary<string, Dictionary<string, ModelMask>> _rawMasks = [];
    private List<JsonArray> _warps = [];
    private JsonArray? _liquifyDrag;
    private (double X, double Y) _liquifyAt;

    public string[] LiquifyModes { get; } = [I18n.T("推动"), I18n.T("膨胀"), I18n.T("收缩")];
    public ObservableCollection<ReplaceChoice> BackgroundChoices { get; } = [];
    public ObservableCollection<ReplaceChoice> SkyChoices { get; } = [];
    public SliderGroup BackgroundGroup { get; private set; } = null!;
    public SliderGroup SkyGroup { get; private set; } = null!;

    [ObservableProperty]
    private double _liquifyRadius = 0.06;

    [ObservableProperty]
    private double _liquifyStrength = 0.7;

    [ObservableProperty]
    private int _liquifyMode;

    [ObservableProperty]
    private ReplaceChoice? _selectedBackground;

    [ObservableProperty]
    private ReplaceChoice? _selectedSky;

    [ObservableProperty]
    private PhotoItem? _matchReference;

    [ObservableProperty]
    private string _fullStatus = "";

    public bool IsLiquify => Tool == RetouchTool.Liquify;
    public bool HasWarps => _warps.Count > 0;
    public bool HasBackground => _edit.GetValueOrDefault("backgroundAsset") > 0;
    public bool HasSky => _edit.GetValueOrDefault("skyAsset") > 0;

    private void MakeFullSliders()
    {
        BackgroundGroup = new SliderGroup(Edits.BackgroundReplace.Title, Edits.BackgroundReplace.Sliders.Select(Make).ToList());
        SkyGroup = new SliderGroup(Edits.SkyReplace.Title, Edits.SkyReplace.Sliders.Select(Make).ToList());
        RefreshReplaceChoices();
    }

    private void RefreshReplaceChoices()
    {
        _showingEdit = true;
        foreach (var (list, sky) in new[] { (BackgroundChoices, false), (SkyChoices, true) })
        {
            list.Clear();
            list.Add(new ReplaceChoice(0, I18n.T("无")));
            foreach (var choice in Replacements.BuiltInChoices(sky).Concat(_replacements.Choices(sky))) list.Add(choice);
        }
        _showingEdit = false;
    }

    /// <summary>Called by ShowEdit, so the pickers follow undo and other photos.</summary>
    private void ShowFullEdit()
    {
        SelectedBackground = BackgroundChoices.FirstOrDefault(choice => choice.Id == (int)_edit.GetValueOrDefault("backgroundAsset")) ?? BackgroundChoices.FirstOrDefault();
        SelectedSky = SkyChoices.FirstOrDefault(choice => choice.Id == (int)_edit.GetValueOrDefault("skyAsset")) ?? SkyChoices.FirstOrDefault();
        OnPropertyChanged(nameof(HasBackground));
        OnPropertyChanged(nameof(HasSky));
        OnPropertyChanged(nameof(HasWarps));
    }

    // ---- Liquify ----

    private JsonArray FlatWarps() => new(_warps.SelectMany(group => group).Select(stroke => stroke!.DeepClone()).ToArray());

    private void LiquifyPress(double px, double py)
    {
        _liquifyDrag = [];
        _liquifyAt = (px, py);
        _drawing = true;
    }

    private async Task LiquifyMoveAsync(double px, double py)
    {
        if (_liquifyDrag is not { } drag) return;
        var (fromX, fromY) = _liquifyAt;
        if (Math.Sqrt(Math.Pow(px - fromX, 2) + Math.Pow(py - fromY, 2)) < MinLiquifyStep) return;
        _liquifyAt = (px, py);
        var (width, height) = _viewport;
        var stroke = (await _engine.CallAsync("liquify", new
        {
            fromX = fromX / width, fromY = fromY / height, toX = px / width, toY = py / height,
            stageWidth = width, stageHeight = height, radius = LiquifyRadius, strength = LiquifyStrength, mode = LiquifyMode,
        })).Json;
        if (stroke is JsonObject found) drag.Add(found.DeepClone());
        RequestRender();
    }

    private void LiquifyRelease()
    {
        if (_liquifyDrag is not { Count: > 0 } drag || SelectedPhoto is null)
        {
            _liquifyDrag = null;
            return;
        }
        _warps.Add(drag);
        _liquifyDrag = null;
        _retouch.SaveWarps(SelectedPhoto.Id, _warps);
        OnPropertyChanged(nameof(HasWarps));
        _thumbnailTimer.Stop();
        _thumbnailTimer.Start();
    }

    /// <summary>Takes back the last liquify drag.</summary>
    [RelayCommand]
    private Task UndoLiquify() => ReplaceWarpsAsync(_warps.Take(Math.Max(0, _warps.Count - 1)).ToList());

    [RelayCommand]
    private Task ClearLiquify() => ReplaceWarpsAsync([]);

    private async Task ReplaceWarpsAsync(List<JsonArray> warps)
    {
        if (SelectedPhoto is null || !_imageOpen) return;
        _warps = warps;
        _retouch.SaveWarps(SelectedPhoto.Id, warps);
        await _engine.CallRawAsync("setWarpStrokes", FlatWarps().ToJsonString());
        OnPropertyChanged(nameof(HasWarps));
        RequestRender();
        _thumbnailTimer.Stop();
        _thumbnailTimer.Start();
    }

    // ---- Background and sky ----

    partial void OnSelectedBackgroundChanged(ReplaceChoice? value) => ChooseReplacement(value, sky: false);

    partial void OnSelectedSkyChanged(ReplaceChoice? value) => ChooseReplacement(value, sky: true);

    private void ChooseReplacement(ReplaceChoice? choice, bool sky)
    {
        if (_showingEdit || choice is null || !_imageOpen) return;
        var key = sky ? "skyAsset" : "backgroundAsset";
        if ((int)_edit.GetValueOrDefault(key) == choice.Id) return;
        var turningOn = choice.Id > 0 && _edit.GetValueOrDefault(key) <= 0;
        var next = With((key, choice.Id, 0));
        if (turningOn && sky && !_edit.ContainsKey("skyRelight")) next["skyRelight"] = DefaultRelight;
        ApplyEdit(next, sky ? I18n.T("换天") : I18n.T("换背景"));
    }

    /// <summary>Whether a change to the edit needs new masks or images in the engine.</summary>
    private static bool ReplacementChanged(IReadOnlyDictionary<string, double> before, IReadOnlyDictionary<string, double> after) =>
        before.GetValueOrDefault("backgroundAsset") != after.GetValueOrDefault("backgroundAsset")
        || before.GetValueOrDefault("skyAsset") != after.GetValueOrDefault("skyAsset")
        || (before.GetValueOrDefault("backgroundExposure") == 0) != (after.GetValueOrDefault("backgroundExposure") == 0);

    private async Task UpdateReplacementAsync()
    {
        if (SelectedPhoto is not { } photo || !_imageOpen) return;
        var ticket = _openTicket;
        AiBusy = true;
        FullStatus = I18n.T("正在识别…");
        try
        {
            await ApplyReplaceAsync(_engine, photo.Id, _edit, _proxySize);
            if (ticket != _openTicket) return;
            FullStatus = "";
            RequestRender();
            _thumbnailTimer.Stop();
            _thumbnailTimer.Start();
        }
        catch (Exception exception)
        {
            FullStatus = I18n.T("主体识别失败：{message}", ("message", Messages.Friendly(exception)));
        }
        finally
        {
            AiBusy = false;
        }
    }

    /// <summary>Sends the subject or sky mask and the replacement images an edit needs.</summary>
    private async Task ApplyReplaceAsync(EngineClient engine, string id, IReadOnlyDictionary<string, double> edit, (int Width, int Height) proxy)
    {
        var background = (int)edit.GetValueOrDefault("backgroundAsset");
        var sky = (int)edit.GetValueOrDefault("skyAsset");
        if ((background > 0 || edit.GetValueOrDefault("backgroundExposure") != 0) && AiModels.Has(AiModels.Matte.File))
        {
            var mask = await RawMaskAsync(engine, id, "subject", proxy);
            await engine.CallAsync("setMask", new { width = mask.Width, height = mask.Height }, mask.Mask);
        }
        if (sky > 0 && AiModels.Has(AiModels.Sky.File))
        {
            var mask = await RawMaskAsync(engine, id, "sky", proxy);
            await engine.CallAsync("setSkyMask", new { width = mask.Width, height = mask.Height }, mask.Mask);
        }
        foreach (var (asset, isSky) in new[] { (background, false), (sky, true) })
        {
            if (asset <= 0) continue;
            var image = await Task.Run(() => _replacements.Resolve(asset));
            if (image is not null) await engine.CallAsync("setReplaceImage", new { sky = isSky, width = image.Width, height = image.Height }, image.Rgba);
        }
    }

    private async Task<ModelMask> RawMaskAsync(EngineClient engine, string id, string model, (int Width, int Height) proxy)
    {
        if (_rawMasks.TryGetValue(id, out var known) && known.TryGetValue(model, out var mask)) return mask;
        await FindModelMaskAsync(engine, id, model, proxy);
        return _rawMasks[id][model];
    }

    /// <summary>Copies an image into the library and uses it as the background or sky.</summary>
    public void ImportReplacement(string path, bool sky)
    {
        try
        {
            var item = _replacements.Import(path, sky);
            RefreshReplaceChoices();
            if (sky) SelectedSky = SkyChoices.First(choice => choice.Id == item.Id);
            else SelectedBackground = BackgroundChoices.First(choice => choice.Id == item.Id);
        }
        catch (Exception exception) when (exception is IOException or ArgumentException or NotSupportedException or UnauthorizedAccessException)
        {
            FullStatus = I18n.T("无法读取这张图片");
        }
    }

    // ---- Colour match ----

    /// <summary>Gives the open photo the tones and colours of another photo in the library.</summary>
    [RelayCommand]
    private Task MatchFromPhoto() => MatchReference is { } photo ? MatchColoursAsync(photo, null) : Task.CompletedTask;

    /// <summary>Gives the open photo the tones and colours of an image file, RAW included.</summary>
    public Task MatchFromFileAsync(string path) => MatchColoursAsync(null, path);

    private async Task MatchColoursAsync(PhotoItem? reference, string? path)
    {
        if (!_imageOpen || SelectedPhoto is null) return;
        var ticket = _openTicket;
        var name = Path.GetFileNameWithoutExtension(reference?.Name ?? path ?? "");
        AiBusy = true;
        FullStatus = I18n.T("追色中…");
        try
        {
            var target = await WithBackgroundEngine(async engine =>
            {
                if (reference is not null)
                {
                    // As the reference looks now, with its own edit.
                    await LoadForBackgroundAsync(engine, reference, ThumbnailProxyLongEdge);
                }
                else
                {
                    await PhotoLoader.DecodeAsync(engine, path!);
                    await engine.CallAsync("buildProxy", new { maxLongEdge = ThumbnailProxyLongEdge });
                    await engine.CallRawAsync("setAdjustments", Edits.ToEngineJson(new Dictionary<string, double>()));
                    await engine.CallAsync("setLook", new { size = 0 });
                }
                return (await engine.CallAsync("framedThumbnail", new { maxLongEdge = MatchLongEdge })).Data;
            });
            // This photo is measured as it is without a look, since the match replaces it.
            await _engine.CallAsync("setLook", new { size = 0 });
            var source = (await _engine.CallAsync("thumbnail", new { maxLongEdge = MatchLongEdge })).Data;
            var lut = await Task.Run(() => ColourMatch.Match(source, 3, target, 3));
            var look = _looks.Add(I18n.T("追色 {name}", ("name", name)), lut);
            _showingEdit = true;
            RefreshLookChoices();
            _showingEdit = false;
            if (ticket != _openTicket) return;
            FullStatus = "";
            ApplyEdit(With(("lutId", look.Id, 0), ("lutAmount", 1, 0)), look.Name);
        }
        catch (Exception exception)
        {
            _showingEdit = false;
            FullStatus = I18n.T("追色失败：{message}", ("message", Messages.Friendly(exception)));
            if (ticket == _openTicket) await SetLookAsync(_engine, (int)_edit.GetValueOrDefault("lutId"));
        }
        finally
        {
            AiBusy = false;
        }
    }
}
