using System.Collections.ObjectModel;
using System.Text.Json.Nodes;
using Avalonia;
using Avalonia.Media.Imaging;
using Avalonia.Platform;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

public sealed partial class PhotoItem(string path) : ObservableObject
{
    public string Path { get; } = path;
    public string Name { get; } = System.IO.Path.GetFileName(path);
    public string Id { get; } = EditStore.IdOf(path);

    /// <summary>Set when the background thumbnailer could not read the file, so it is not retried.</summary>
    internal bool ThumbnailFailed { get; set; }

    /// <summary>Set when the edit changed without the photo being open, so its thumbnail is redrawn.</summary>
    internal bool ThumbnailStale { get; set; }

    internal bool NeedsThumbnail => (Thumbnail is null || ThumbnailStale) && !ThumbnailFailed;

    [ObservableProperty]
    private Bitmap? _thumbnail;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Stars))]
    private int _rating;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsPicked), nameof(IsRejected))]
    private int _flag;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(LabelBrush), nameof(HasLabel))]
    private int _label;

    /// <summary>1-based smart group, or 0 before the photos are analysed.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(GroupText))]
    private int _group;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(GroupText))]
    private bool _isBest;

    public string Stars => Rating == 0 ? "" : new string('★', Rating);
    public bool IsPicked => Flag == CullStore.FlagPick;
    public bool IsRejected => Flag == CullStore.FlagReject;
    public bool HasLabel => Label > 0;
    public Avalonia.Media.IBrush LabelBrush => Label > 0
        ? new Avalonia.Media.SolidColorBrush(Avalonia.Media.Color.Parse(ColourLabels.All[Label - 1].Colour))
        : Avalonia.Media.Brushes.Transparent;
    public string GroupText => Group == 0 ? "" : IsBest ? I18n.T("组 {group} · 最佳", ("group", Group)) : I18n.T("组 {group}", ("group", Group));
}

public sealed partial class SliderItem(SliderSpec spec, Action<SliderItem> changed) : ObservableObject
{
    public SliderSpec Spec { get; } = spec;
    public string Label => Spec.Label;
    public double Min => Spec.Min;
    public double Max => Spec.Max;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Display), nameof(Changed))]
    private double _value = spec.Resting;

    /// <summary>False for a setting that means nothing in the current state, such as printer lights for a slide.</summary>
    [ObservableProperty]
    private bool _isVisible = true;

    public string Display => Spec.Max > 10 ? Value.ToString("0") : Value.ToString("0.00");
    public bool Changed => Value != Spec.Resting;

    /// <summary>False while the view model sets values itself, so that is not taken as an edit.</summary>
    internal bool Notify { get; set; } = true;

    partial void OnValueChanged(double value)
    {
        if (Notify) changed(this);
    }

    [RelayCommand]
    private void Reset() => Value = Spec.Resting;
}

public sealed record SliderGroup(string Title, IReadOnlyList<SliderItem> Sliders);

/// <summary>One colour-grading wheel: hue in degrees and amount 0..1 for a tonal range.</summary>
public sealed partial class GradeItem(string range, string label) : ObservableObject
{
    public string Range { get; } = range;
    public string Label { get; } = label;
    public string HueKey => $"grade{Range}Hue";
    public string AmountKey => $"grade{Range}Amount";

    [ObservableProperty]
    private double _hue;

    [ObservableProperty]
    private double _amount;
}

public sealed partial class HistoryRow(int index, string label) : ObservableObject
{
    public int Index { get; } = index;

    [ObservableProperty]
    private string _label = label;
}

public enum Picker { None, WhiteBalance, Point }

public sealed partial class MainViewModel : ObservableObject, IDisposable
{
    private const int ProxyLongEdge = 2048;
    private const int ThumbnailLongEdge = 160;
    /// <summary>Background thumbnails only need a small proxy, which keeps them quick.</summary>
    private const int ThumbnailProxyLongEdge = 512;
    private const int HistogramLongEdge = 256;
    private const double MaxZoomRatio = 4;
    /// <summary>Past the right edge, so the whole frame shows the unedited image.</summary>
    private const double CompareAll = 1.01;

    public static readonly (string Label, double? Ratio)[] CropRatios =
        [(I18n.T("自由"), null), ("1:1", 1), ("4:5", 4 / 5.0), ("3:4", 3 / 4.0), ("2:3", 2 / 3.0), ("5:7", 5 / 7.0), ("3:2", 3 / 2.0), ("16:9", 16 / 9.0)];

    private readonly EngineClient _engine = new();
    private readonly Lazy<EngineClient> _thumbnailer = new(() => new EngineClient());
    /// <summary>The background engine handles one photo at a time for thumbnails, analysis and batch export.</summary>
    private readonly SemaphoreSlim _backgroundTurn = new(1, 1);
    private readonly CullStore _cull = new();
    private readonly PresetStore _presets = new();
    private readonly ProjectStore _projects = new();
    /// <summary>Version 2 judges faces; earlier results, made without them, are measured again.</summary>
    private readonly JsonFile<Dictionary<string, PhotoAnalysis>> _analysisFile = new("analyses-v2.json");
    private readonly JsonFile<ProofSettings> _proofFile = new("proof.json");
    private readonly EditStore _store = new();
    private readonly LookLibrary _looks = new();
    private readonly JsonFile<OutputSettings> _outputFile = new("output.json");
    private readonly Dictionary<string, EditHistory> _histories = [];
    private readonly Dictionary<string, SliderItem> _sliders = [];
    private readonly DispatcherTimer _settleTimer = new() { Interval = TimeSpan.FromMilliseconds(300) };
    private readonly DispatcherTimer _thumbnailTimer = new() { Interval = TimeSpan.FromMilliseconds(900) };
    private readonly DispatcherTimer _detailTimer = new() { Interval = TimeSpan.FromMilliseconds(250) };

    private Dictionary<string, double> _edit = [];
    private EditHistory? _history;
    private JsonNode? _info;
    private (int Width, int Height) _viewport = (1, 1);
    private double _scaling = 1;
    private View _view = View.Fit;
    private bool _rendering;
    private bool _renderAgain;
    private bool _editChanged;
    private bool _imageOpen;
    private bool _showingEdit;
    private bool _fillingThumbnails;
    private bool _comparingAll;
    private int _openTicket;
    private WriteableBitmap? _spareFrame;

    public MainViewModel()
    {
        SliderGroup Group(SliderGroupSpec spec) => new(spec.Title, spec.Sliders.Select(Make).ToList());
        WhiteBalanceGroup = Group(Edits.WhiteBalance);
        ToneGroup = Group(Edits.Tone);
        ParametricGroup = Group(Edits.Parametric);
        EffectsGroup = Group(Edits.Effects);
        CalibrationGroup = Group(Edits.Calibration);
        LensGroup = Group(Edits.Lens);
        PerspectiveGroup = Group(Edits.Perspective);
        PointGroup = Group(Edits.Point);
        HslAll = Edits.HslSliders().Select(Make).ToList();
        LookAmount = Make(Edits.LookAmount);
        RotateSlider = Make(Edits.Rotate);
        MakeLocalSliders();
        MakeFullSliders();
        MakePortraitSliders();
        MakeFilmSliders();
        Groups = [WhiteBalanceGroup, ToneGroup, ParametricGroup, EffectsGroup, CalibrationGroup, LensGroup, PerspectiveGroup, PointGroup, new("HSL", HslAll), new(I18n.T("其他"), [LookAmount, RotateSlider])];
        Grades = Edits.GradeRanges.Select(range => new GradeItem(range.Key, range.Label)).ToList();
        RefreshLookChoices();
        Output = _outputFile.Load();
        Photos.CollectionChanged += (_, _) =>
        {
            OnPropertyChanged(nameof(HasPhotos));
            OnPropertyChanged(nameof(PhotoCount));
            OnPropertyChanged(nameof(CanResume));
        };
        SelectedPhotos.CollectionChanged += (_, _) => OnSelectionChanged();
        RefreshPresetNames();
        RefreshProjects();

        _settleTimer.Tick += (_, _) =>
        {
            _settleTimer.Stop();
            _ = RefreshHistogramAsync();
        };
        _thumbnailTimer.Tick += (_, _) =>
        {
            _thumbnailTimer.Stop();
            if (SelectedPhoto is { } photo) _ = RefreshThumbnailAsync(photo, _openTicket);
        };
        _detailTimer.Tick += (_, _) =>
        {
            _detailTimer.Stop();
            _ = UpdateDetailAsync();
        };
        _ = StartAsync();
    }

    private SliderItem Make(SliderSpec spec) => _sliders[spec.Key] = new SliderItem(spec, OnSliderChanged);

    public ObservableCollection<PhotoItem> Photos { get; } = [];

    /// <summary>Every slider, grouped; panels show some groups directly.</summary>
    public IReadOnlyList<SliderGroup> Groups { get; }
    public SliderGroup WhiteBalanceGroup { get; }
    public SliderGroup ToneGroup { get; }
    public SliderGroup ParametricGroup { get; }
    public SliderGroup EffectsGroup { get; }
    public SliderGroup CalibrationGroup { get; }
    public SliderGroup LensGroup { get; }
    public SliderGroup PerspectiveGroup { get; }
    public SliderGroup PointGroup { get; }
    public IReadOnlyList<SliderItem> HslAll { get; }
    public SliderItem LookAmount { get; }
    public SliderItem RotateSlider { get; }
    public IReadOnlyList<GradeItem> Grades { get; }
    public string[] HslBandNames { get; } = Edits.HslBands.Select(band => band.Name).ToArray();
    public string[] CurveChannelNames { get; } = ["RGB", I18n.T("红"), I18n.T("绿"), I18n.T("蓝")];
    public string[] CropRatioNames { get; } = CropRatios.Select(ratio => ratio.Label).ToArray();
    public string[] CameraProfiles { get; } = [I18n.T("标准"), I18n.T("相机匹配")];
    public ObservableCollection<LookInfo> LookChoices { get; } = [];
    public ObservableCollection<HistoryRow> HistoryRows { get; } = [];
    public ObservableCollection<Snapshot> Snapshots { get; } = [];
    public OutputSettings Output { get; private set; }

    public SliderItem Slider(string key) => _sliders[key];

    /// <summary>The edit as it stands, for controls that draw from it.</summary>
    public IReadOnlyDictionary<string, double> CurrentEdit => _edit;

    /// <summary>Size and type of the open photo, for the panel header.</summary>
    [ObservableProperty]
    private string _photoInfo = "";

    public bool HasPhotos => Photos.Count > 0;

    /// <summary>Interface languages by their own names; a new choice applies from the next start.</summary>
    public string[] LanguageNames { get; } = I18n.Languages.Select(language => language.Name).ToArray();

    public int LanguageIndex
    {
        get => Math.Max(0, Array.FindIndex(I18n.Languages, language => language.Code == (_chosenLanguage ?? I18n.Language)));
        set
        {
            if (value < 0 || value >= I18n.Languages.Length || value == LanguageIndex) return;
            _chosenLanguage = I18n.Languages[value].Code;
            I18n.Choose(_chosenLanguage);
            OnPropertyChanged();
            Status = I18n.T("界面语言已更改，重新启动后生效");
        }
    }

    private string? _chosenLanguage;
    public string PhotoCount => Photos.Count == 0 ? "" : I18n.T("{count} 张", ("count", Photos.Count));

    [ObservableProperty]
    private Bitmap? _preview;

    [ObservableProperty]
    private string _status = I18n.T("正在启动引擎…");

    /// <summary>The engine's own wording of the last error, shown as a tooltip.</summary>
    [ObservableProperty]
    private string? _statusDetail;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(AutoToneCommand), nameof(ResetCommand), nameof(UndoCommand), nameof(RedoCommand), nameof(TakeSnapshotCommand))]
    [NotifyPropertyChangedFor(nameof(HasOthersSelected), nameof(CanAddLayer))]
    private bool _canEdit;

    [ObservableProperty]
    private HistogramData? _histogram;

    [ObservableProperty]
    private bool _clipShadows;

    [ObservableProperty]
    private bool _clipHighlights;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HslSliders))]
    private int _hslBand;

    public IReadOnlyList<SliderItem> HslSliders => HslAll.Skip(HslBand * 3).Take(3).ToList();

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(CurveChannel))]
    private int _curveChannelIndex;

    public string CurveChannel => Curves.Channels[Math.Clamp(CurveChannelIndex, 0, 3)];

    [ObservableProperty]
    private bool _pointActive;

    [ObservableProperty]
    private Avalonia.Media.IBrush _pointSwatch = Avalonia.Media.Brushes.Gray;

    [ObservableProperty]
    private Picker _picker;

    [ObservableProperty]
    private string _toolHint = "";

    [ObservableProperty]
    private bool _cropEditing;

    [ObservableProperty]
    private int _cropRatioIndex;

    [ObservableProperty]
    private bool _compareOn;

    [ObservableProperty]
    private double _compareSplit = 0.5;

    [ObservableProperty]
    private string _zoomLabel = I18n.T("适合");

    [ObservableProperty]
    private string _lensStatus = "";

    [ObservableProperty]
    private bool _cameraProfileAvailable;

    [ObservableProperty]
    private bool _lensAutoAvailable;

    [ObservableProperty]
    private int _historyIndex = -1;

    [ObservableProperty]
    private LookInfo? _selectedLook;

    /// <summary>Raised when the crop overlay or the compare divider should be redrawn.</summary>
    public event Action? OverlayChanged;

    public bool IsZoomed => _view.Zoom > 1;
    public bool PickingWhiteBalance => Picker == Picker.WhiteBalance;
    public bool PickingPoint => Picker == Picker.Point;

    partial void OnPickerChanged(Picker value)
    {
        if (value != Picker.None) Tool = RetouchTool.None;
        OnPropertyChanged(nameof(PickingWhiteBalance));
        OnPropertyChanged(nameof(PickingPoint));
        ToolHint = value switch
        {
            Picker.WhiteBalance => I18n.T("点击画面中应为中性灰或白色的地方"),
            Picker.Point => I18n.T("点击画面中要调整的颜色"),
            _ => "",
        };
    }

    private async Task StartAsync()
    {
        try
        {
            await _engine.Ready;
            Status = I18n.T("打开照片开始修图");
            _ = LoadProfilesAsync();
        }
        catch (Exception exception)
        {
            Status = I18n.T("引擎无法启动：{message}", ("message", exception.Message));
        }
    }

    // ---- Library ----

    public void AddPhotos(IEnumerable<string> paths)
    {
        var known = Photos.Select(photo => photo.Path).ToHashSet();
        PhotoItem? first = null;
        foreach (var path in paths.Where(path => !known.Contains(path) && File.Exists(path)))
        {
            known.Add(path);
            var photo = new PhotoItem(path) { Rating = _cull.Rating(EditStore.IdOf(path)), Flag = _cull.Flag(EditStore.IdOf(path)), Label = _cull.Label(EditStore.IdOf(path)) };
            Photos.Add(photo);
            first ??= photo;
        }
        ApplyGroups();
        RefreshVisible();
        if (first is not null) SelectedPhoto = first;
        SaveSession();
        _ = FillThumbnailsAsync();
    }

    /// <summary>Adds every photo in a folder, in name order. Returns how many it found.</summary>
    public int AddFolder(string folder)
    {
        var files = Directory.EnumerateFiles(folder).Where(PhotoLoader.IsPhoto).Order(StringComparer.OrdinalIgnoreCase).ToList();
        AddPhotos(files);
        if (files.Count == 0) Status = I18n.T("这个文件夹里没有可以打开的照片");
        return files.Count;
    }

    /// <summary>Files and folders dropped on the window.</summary>
    public void AddDropped(IEnumerable<string> paths)
    {
        var files = new List<string>();
        foreach (var path in paths)
        {
            if (Directory.Exists(path)) files.AddRange(Directory.EnumerateFiles(path).Where(PhotoLoader.IsPhoto).Order(StringComparer.OrdinalIgnoreCase));
            else if (PhotoLoader.IsPhoto(path)) files.Add(path);
        }
        if (files.Count == 0) Status = I18n.T("拖入的文件里没有可以打开的照片");
        else AddPhotos(files);
    }

    public void SelectNext(int step)
    {
        if (Photos.Count == 0) return;
        var index = SelectedPhoto is null ? 0 : Photos.IndexOf(SelectedPhoto) + step;
        SelectedPhoto = Photos[Math.Clamp(index, 0, Photos.Count - 1)];
    }

    /// <summary>Makes thumbnails for photos not opened yet, on a second engine so editing is not held up.</summary>
    private async Task FillThumbnailsAsync()
    {
        if (_fillingThumbnails) return;
        _fillingThumbnails = true;
        try
        {
            // The open photo gets its thumbnail from the main engine.
            while (Photos.FirstOrDefault(photo => photo.NeedsThumbnail && photo != SelectedPhoto) is { } photo)
            {
                try
                {
                    var stale = photo.ThumbnailStale;
                    var bitmap = await WithBackgroundEngine(async engine =>
                    {
                        await LoadForBackgroundAsync(engine, photo, ThumbnailProxyLongEdge);
                        return await FramedThumbnailAsync(engine);
                    });
                    // The photo may have been opened, and given a fresher thumbnail, meanwhile.
                    if (stale || photo.Thumbnail is null) photo.Thumbnail = bitmap;
                    photo.ThumbnailStale = false;
                }
                catch (Exception)
                {
                    photo.ThumbnailFailed = true;
                }
            }
        }
        catch (Exception)
        {
            // Without a second engine, photos get their thumbnail when opened.
        }
        finally
        {
            _fillingThumbnails = false;
        }
    }

    private static async Task<Bitmap?> FramedThumbnailAsync(EngineClient engine)
    {
        var size = await engine.CallAsync("framedThumbnailSize", new { maxLongEdge = ThumbnailLongEdge });
        var rgb = (await engine.CallAsync("framedThumbnail", new { maxLongEdge = ThumbnailLongEdge })).Data;
        return size.Json is null ? null : BitmapFromRgb(rgb, (int)size.Json["width"]!, (int)size.Json["height"]!);
    }

    // ---- Opening ----

    private async Task OpenAsync(PhotoItem photo)
    {
        var ticket = ++_openTicket;
        CanEdit = false;
        _imageOpen = false;
        // Nothing from the previous photo may stay on screen while this one opens or if it fails.
        Preview = null;
        _spareFrame = null;
        Histogram = null;
        PhotoInfo = "";
        Picker = Picker.None;
        CropEditing = false;
        StatusDetail = null;
        Status = I18n.T("正在打开 {name}…", ("name", photo.Name));
        try
        {
            var started = Environment.TickCount64;
            var info = await PhotoLoader.DecodeAsync(_engine, photo.Path);
            var proxy = (await _engine.CallAsync("buildProxy", new { maxLongEdge = ProxyLongEdge })).Json!;
            await PhotoLoader.ApplyLensDatabaseAsync(_engine, info);
            if (ticket != _openTicket) return;
            _info = info;
            _edit = _store.Load(photo.Id);
            if (!_histories.TryGetValue(photo.Id, out var history)) _histories[photo.Id] = history = new EditHistory(_edit, _retouch.Locals(photo.Id));
            _history = history;
            _edit = history.Current;
            _view = View.Fit;
            ShowEdit();
            SyncHistory();
            ShowSnapshots();
            ShowCameraAndLens();
            Tool = RetouchTool.None;
            await LoadRetouchAsync(photo, proxy);
            await _engine.CallRawAsync("setAdjustments", Edits.ToEngineJson(_edit));
            await SetLookAsync(_engine, _edit);
            await _engine.CallAsync("setView", new { zoom = 1.0, x = 0.5, y = 0.5 });
            await _engine.CallAsync("setCompare", new { split = CompareOn ? CompareSplit : 0 });
            await _engine.CallAsync("setClipWarnings", new { highlights = ClipHighlights, shadows = ClipShadows });
            if (ProofOn) await SendProofAsync();
            if (ticket != _openTicket) return;
            _imageOpen = true;
            CanEdit = true;
            UpdateZoomLabel();
            var kind = info["isRaw"]?.GetValue<bool>() == true ? "RAW" : System.IO.Path.GetExtension(photo.Name).TrimStart('.').ToUpperInvariant();
            PhotoInfo = $"{(int)info["width"]!} × {(int)info["height"]!} · {kind}";
            Status = $"{photo.Name}，{(int)info["width"]!} × {(int)info["height"]!}，{Environment.TickCount64 - started} ms";
            RequestRender();
            _ = RefreshHistogramAsync();
            await RefreshThumbnailAsync(photo, ticket);
        }
        catch (Exception exception)
        {
            if (ticket != _openTicket) return;
            _imageOpen = false;
            _info = null;
            Preview = null;
            Status = I18n.T("无法打开 {name}：{exception}", ("name", photo.Name), ("exception", Messages.Friendly(exception)));
            StatusDetail = Messages.Detail(exception);
            photo.ThumbnailFailed = true;
        }
    }

    private void ShowCameraAndLens()
    {
        var isRaw = _info?["isRaw"]?.GetValue<bool>() == true;
        CameraProfileAvailable = _info?["cameraMatch"]?.GetValue<bool>() == true;
        LensAutoAvailable = _info?["lens"] is not null;
        OnPropertyChanged(nameof(CameraProfileIndex));
        OnPropertyChanged(nameof(LensAuto));
        var lensName = _info?["lensQuery"]?["lens"]?.GetValue<string>();
        var camera = !isRaw ? "" : CameraProfileAvailable ? I18n.T("已按机内 JPEG 拟合相机色彩。") : I18n.T("默认色彩已接近机内 JPEG，无需相机匹配。");
        var lens = _info?["lensSource"]?.GetValue<string>() == "lensfun"
            ? I18n.T("Lensfun 镜头库有 {lens} 的校正数据（CC BY-SA 3.0），勾选上方选项后生效。", ("lens", lensName))
            : LensAutoAvailable ? I18n.T("文件内含镜头数据。")
            : lensName is not null ? I18n.T("镜头库里没有 {lens} 的数据，可用下方滑块手动校正。", ("lens", lensName))
            : I18n.T("文件没有可用的镜头数据，可用下方滑块手动校正。");
        LensStatus = (camera + lens).Trim();
    }

    private async Task RefreshThumbnailAsync(PhotoItem photo, int ticket)
    {
        try
        {
            var bitmap = await FramedThumbnailAsync(_engine);
            if (ticket == _openTicket && bitmap is not null) photo.Thumbnail = bitmap;
        }
        catch (EngineException)
        {
            // A thumbnail is not worth an error message; the next edit tries again.
        }
    }

    private async Task RefreshHistogramAsync()
    {
        if (!_imageOpen) return;
        var ticket = _openTicket;
        try
        {
            var rgb = (await _engine.CallAsync("histogramThumbnail", new { maxLongEdge = HistogramLongEdge })).Data;
            if (ticket == _openTicket && _imageOpen) Histogram = HistogramData.FromRgb(rgb);
        }
        catch (EngineException)
        {
            // The histogram is a guide; a missed update is replaced by the next one.
        }
    }

    // ---- Editing ----

    /// <summary>Sets every control from the current edit without treating that as an edit.</summary>
    private void ShowEdit()
    {
        _showingEdit = true;
        try
        {
            foreach (var slider in _sliders.Values)
            {
                slider.Notify = false;
                slider.Value = _edit.TryGetValue(slider.Spec.Key, out var value) ? value : slider.Spec.Resting;
                slider.Notify = true;
            }
            foreach (var grade in Grades)
            {
                grade.Hue = _edit.GetValueOrDefault(grade.HueKey);
                grade.Amount = _edit.GetValueOrDefault(grade.AmountKey);
            }
            var lookId = (int)_edit.GetValueOrDefault("lutId");
            SelectedLook = LookChoices.FirstOrDefault(look => look.Id == lookId) ?? LookChoices[0];
            ShowFilm();
            PointActive = _edit.GetValueOrDefault("pointWidth") > 0;
            PointSwatch = new Avalonia.Media.SolidColorBrush(ColorWheel.HsvColour(_edit.GetValueOrDefault("pointHue"), 0.75, 0.85));
            ShowFullEdit();
            OnPropertyChanged(nameof(CurrentEdit));
            OnPropertyChanged(nameof(CameraProfileIndex));
            OnPropertyChanged(nameof(LensAuto));
            OnPropertyChanged(nameof(PerspectiveKeep));
        }
        finally
        {
            _showingEdit = false;
        }
        OverlayChanged?.Invoke();
    }

    /// <summary>Makes next the current edit and records it. label names an action, like a preset.</summary>
    public void ApplyEdit(Dictionary<string, double> next, string? label = null)
    {
        if (!_imageOpen || SelectedPhoto is null || _history is null) return;
        var before = _edit;
        _edit = next;
        _history.Record(before, next, label);
        EditChanged(before);
    }

    /// <summary>Everything that follows a change to the edit, whatever made it.</summary>
    private void EditChanged(IReadOnlyDictionary<string, double> before)
    {
        if (SelectedPhoto is null) return;
        _store.Save(SelectedPhoto.Id, _edit);
        var lookId = (int)_edit.GetValueOrDefault("lutId");
        if (lookId != (int)before.GetValueOrDefault("lutId") || _edit.GetValueOrDefault("filmPrint") != before.GetValueOrDefault("filmPrint"))
        {
            var edit = _edit;
            _ = Fire(() => SetLookAsync(_engine, edit));
        }
        if (ReplacementChanged(before, _edit)) _ = UpdateReplacementAsync();
        if (NeedsSkin(before) != NeedsSkin(_edit)) _ = UpdateSkinAsync();
        else ShowSkinStatus(SelectedPhoto.Id, _edit);
        ShowEdit();
        SyncHistory();
        _editChanged = true;
        RequestRender();
        _settleTimer.Stop();
        _settleTimer.Start();
        _thumbnailTimer.Stop();
        _thumbnailTimer.Start();
        UpdateZoomLabel();
    }

    private Dictionary<string, double> With(params (string Key, double Value, double Resting)[] changes)
    {
        var next = new Dictionary<string, double>(_edit);
        foreach (var (key, value, resting) in changes)
        {
            if (value == resting || !double.IsFinite(value)) next.Remove(key);
            else next[key] = value;
        }
        return next;
    }

    private void OnSliderChanged(SliderItem slider)
    {
        if (!_imageOpen || _showingEdit) return;
        var next = With((slider.Spec.Key, slider.Value, slider.Spec.Resting));
        if (slider.Spec.Key == "rotate")
        {
            var (width, height) = FrameOfImage();
            Frame.SetCrop(next, Frame.ShrinkToImage(Frame.CropOf(next), slider.Value, width, height));
        }
        ApplyEdit(next);
    }

    /// <summary>A colour wheel moved; hue and amount change together.</summary>
    public void SetGrade(GradeItem grade, double hue, double amount)
    {
        if (!_imageOpen) return;
        ApplyEdit(With((grade.HueKey, amount == 0 ? 0 : hue, 0), (grade.AmountKey, amount, 0)));
    }

    public int CameraProfileIndex
    {
        get => CameraProfileAvailable && _edit.GetValueOrDefault("cameraProfile") > 0 ? 1 : 0;
        set
        {
            if (!_showingEdit && value != CameraProfileIndex) ApplyEdit(With(("cameraProfile", value, 0)));
        }
    }

    public bool LensAuto
    {
        get => LensAutoAvailable && _edit.GetValueOrDefault("lensAuto") > 0;
        set
        {
            if (!_showingEdit && value != LensAuto) ApplyEdit(With(("lensAuto", value ? 1 : 0, 0)));
        }
    }

    public bool PerspectiveKeep
    {
        get => _edit.GetValueOrDefault("perspectiveKeep") > 0;
        set
        {
            if (!_showingEdit && value != PerspectiveKeep) ApplyEdit(With(("perspectiveKeep", value ? 1 : 0, 0)));
        }
    }

    partial void OnSelectedLookChanged(LookInfo? value)
    {
        if (_showingEdit || value is null || !_imageOpen) return;
        var amount = value.Id == 0 ? 0 : _edit.GetValueOrDefault("lutAmount") is > 0 and var current ? current : 1;
        ApplyEdit(With(("lutId", value.Id, 0), ("lutAmount", amount, 0)));
    }

    private void RefreshLookChoices()
    {
        LookChoices.Clear();
        LookChoices.Add(new LookInfo(0, "", I18n.T("无")));
        foreach (var look in Looks.BuiltInLooks) LookChoices.Add(look);
        foreach (var look in Looks.FilmLooks) LookChoices.Add(look);
        foreach (var look in _looks.Imported) LookChoices.Add(new LookInfo(look.Id, I18n.T("导入"), look.Name));
    }

    /// <summary>Sets an edit's look: a film simulation for a film look, else its LUT.</summary>
    private async Task SetLookAsync(EngineClient engine, IReadOnlyDictionary<string, double> edit)
    {
        var id = (int)edit.GetValueOrDefault("lutId");
        if (FilmSim.Film(id) is { } film && FilmSim.Call(film, edit) is { } call)
        {
            await engine.CallAsync("setFilm", call.Args, call.Data);
            return;
        }
        var lut = _looks.Resolve(id);
        if (lut is null)
        {
            await engine.CallAsync("setLook", new { size = 0 });
            return;
        }
        var bytes = new byte[lut.Data.Length * 4];
        Buffer.BlockCopy(lut.Data, 0, bytes, 0, bytes.Length);
        await engine.CallAsync("setLook", new { size = lut.Size }, bytes);
    }

    /// <summary>Imports a .cube file and applies it to the open photo.</summary>
    public void ImportLook(string path)
    {
        try
        {
            var look = _looks.Import(path);
            _showingEdit = true;
            RefreshLookChoices();
            _showingEdit = false;
            Status = I18n.T("已导入滤镜 {name}", ("name", look.Name));
            if (_imageOpen) ApplyEdit(With(("lutId", look.Id, 0), ("lutAmount", 1, 0)), I18n.T("滤镜 {name}", ("name", look.Name)));
            else ShowEdit();
        }
        catch (Exception exception) when (exception is FormatException or IOException or UnauthorizedAccessException)
        {
            _showingEdit = false;
            Status = I18n.T("无法导入 LUT：{exception}", ("exception", Messages.Friendly(exception)));
        }
    }

    public void SetCurve(Dictionary<string, double> next)
    {
        if (_imageOpen) ApplyEdit(next);
    }

    [RelayCommand]
    private void ResetCurve() => ApplyEdit(Curves.WithPoints(_edit, CurveChannel, Curves.Identity));

    [RelayCommand]
    private void RemovePointColour() =>
        ApplyEdit(With(("pointHue", 0, 0), ("pointWidth", 0, 0), ("pointHueShift", 0, 0), ("pointSatShift", 0, 0), ("pointLumShift", 0, 0)), I18n.T("移除颜色混合点"));

    [RelayCommand(CanExecute = nameof(CanEdit))]
    private async Task AutoTone()
    {
        try
        {
            var reply = await _engine.CallAsync("autoTone");
            if (reply.Json is not JsonObject tone) return;
            var next = new Dictionary<string, double>(_edit);
            foreach (var (key, value) in tone)
            {
                var number = value?.GetValue<double>() ?? 0;
                if (number == 0) next.Remove(key);
                else next[key] = number;
            }
            ApplyEdit(next, I18n.T("智能调色"));
        }
        catch (EngineException exception)
        {
            Status = I18n.T("智能调色失败：{exception}", ("exception", Messages.Friendly(exception)));
        }
    }

    [RelayCommand(CanExecute = nameof(CanEdit))]
    private void Reset() => ApplyEdit([], I18n.T("全部重置"));

    // ---- History and snapshots ----

    [RelayCommand(CanExecute = nameof(CanEdit))]
    public void Undo() => MoveInHistory(-1);

    [RelayCommand(CanExecute = nameof(CanEdit))]
    public void Redo() => MoveInHistory(1);

    private void MoveInHistory(int step)
    {
        if (_history is null || !_imageOpen) return;
        var target = _history.Index + step;
        if (target < 0 || target >= _history.Entries.Count) return;
        JumpTo(target);
    }

    private void JumpTo(int index)
    {
        if (_history is null) return;
        var before = _edit;
        _edit = _history.MoveTo(index);
        RestoreLayersFromHistory();
        EditChanged(before);
    }

    partial void OnHistoryIndexChanged(int value)
    {
        if (!_showingEdit && _history is not null && value >= 0 && value != _history.Index) JumpTo(value);
    }

    private void SyncHistory()
    {
        if (_history is null) return;
        var entries = _history.Entries;
        while (HistoryRows.Count > entries.Count) HistoryRows.RemoveAt(HistoryRows.Count - 1);
        for (var i = 0; i < entries.Count; i++)
        {
            if (i < HistoryRows.Count) HistoryRows[i].Label = entries[i].Label;
            else HistoryRows.Add(new HistoryRow(i, entries[i].Label));
        }
        _showingEdit = true;
        HistoryIndex = _history.Index;
        _showingEdit = false;
    }

    private void ShowSnapshots()
    {
        Snapshots.Clear();
        if (SelectedPhoto is null) return;
        foreach (var snapshot in _store.Snapshots(SelectedPhoto.Id)) Snapshots.Add(snapshot);
    }

    [RelayCommand(CanExecute = nameof(CanEdit))]
    private void TakeSnapshot()
    {
        if (SelectedPhoto is null) return;
        var name = I18n.T("快照 {number}（{time}）", ("number", Snapshots.Count + 1), ("time", DateTime.Now.ToString("HH:mm")));
        Snapshots.Add(new Snapshot(name, EditStore.Kept(_edit)));
        _store.SaveSnapshots(SelectedPhoto.Id, Snapshots);
        Status = I18n.T("已保存{name}", ("name", name));
    }

    [RelayCommand]
    private void ApplySnapshot(Snapshot snapshot)
    {
        // Framing comes with the snapshot: it is part of how the photo looked then.
        ApplyEdit(new Dictionary<string, double>(snapshot.Edit), I18n.T("应用{name}", ("name", snapshot.Name)));
    }

    [RelayCommand]
    private void DeleteSnapshot(Snapshot snapshot)
    {
        if (SelectedPhoto is null) return;
        Snapshots.Remove(snapshot);
        _store.SaveSnapshots(SelectedPhoto.Id, Snapshots);
    }

    // ---- Viewing ----

    public void SetViewport(int width, int height, double scaling)
    {
        _viewport = (Math.Max(1, width), Math.Max(1, height));
        _scaling = scaling;
        UpdateZoomLabel();
        RequestRender();
        OverlayChanged?.Invoke();
    }

    public double Scaling => _scaling;
    public (int Width, int Height) Viewport => _viewport;

    /// <summary>The image's size after quarter turns, in its own pixels.</summary>
    private (double Width, double Height) FrameOfImage()
    {
        var width = _info?["width"]?.GetValue<double>() ?? 1;
        var height = _info?["height"]?.GetValue<double>() ?? 1;
        return Frame.FrameSize(width, height, (int)_edit.GetValueOrDefault("quarter"));
    }

    private Crop ShownCrop => CropEditing ? Crop.Full : Frame.CropOf(_edit);

    /// <summary>The framed image fitted to the stage, in device pixels.</summary>
    private Box Fitted()
    {
        var (width, height) = FrameOfImage();
        var crop = ShownCrop;
        return Frame.FitBox(width * crop.W / (height * crop.H), _viewport.Width, _viewport.Height);
    }

    /// <summary>Where the image is drawn on the stage now, in device pixels.</summary>
    public Box DisplayBox() => Frame.ViewBox(Fitted(), _viewport.Width, _viewport.Height, CropEditing ? View.Fit : _view);

    /// <summary>The zoom at which one image pixel fills one device pixel, never below fitted.</summary>
    private double ActualPixelsZoom()
    {
        var (width, _) = FrameOfImage();
        return Math.Max(1, width * ShownCrop.W / Math.Max(1, Fitted().Width));
    }

    private void ApplyView(View next)
    {
        _view = next with { X = Math.Clamp(next.X, 0, 1), Y = Math.Clamp(next.Y, 0, 1) };
        _ = Fire(() => _engine.CallAsync("setView", new { zoom = _view.Zoom, x = _view.X, y = _view.Y }));
        UpdateZoomLabel();
        RequestRender();
        _detailTimer.Stop();
        _detailTimer.Start();
        OverlayChanged?.Invoke();
    }

    private void UpdateZoomLabel()
    {
        ZoomLabel = !_imageOpen || _view.Zoom <= 1 ? I18n.T("适合") : $"{Math.Round(_view.Zoom / ActualPixelsZoom() * 100)}%";
        OnPropertyChanged(nameof(IsZoomed));
    }

    /// <summary>Zooms while keeping the point at stage position (px, py), in device pixels, where it is.</summary>
    public void ZoomAround(double zoom, double px, double py)
    {
        if (!_imageOpen || CropEditing) return;
        var limited = Math.Min(Math.Max(1, ActualPixelsZoom() * MaxZoomRatio), Math.Max(1, zoom));
        var box = DisplayBox();
        var (s, t) = ((px - box.Left) / box.Width, (py - box.Top) / box.Height);
        var fit = Fitted();
        var (width, height) = (fit.Width * limited, fit.Height * limited);
        ApplyView(new View(limited, s + (_viewport.Width / 2.0 - px) / width, t + (_viewport.Height / 2.0 - py) / height));
    }

    public void ZoomBy(double factor, double px, double py) => ZoomAround(_view.Zoom * factor, px, py);

    public void ZoomToRatio(double ratio) => ZoomAround(ActualPixelsZoom() * ratio, _viewport.Width / 2.0, _viewport.Height / 2.0);

    [RelayCommand]
    public void ToggleZoom()
    {
        if (!_imageOpen || CropEditing) return;
        if (_view.Zoom > 1) ApplyView(View.Fit);
        else ZoomToRatio(1);
    }

    [RelayCommand]
    private void FitView() => ApplyView(View.Fit);

    [RelayCommand]
    private void ActualPixels() => ZoomToRatio(1);

    public View CurrentView => _view;

    /// <summary>Pans from a view by a drag of (dx, dy) device pixels.</summary>
    public void PanFrom(View start, double dx, double dy)
    {
        if (start.Zoom <= 1) return;
        var fit = Fitted();
        ApplyView(new View(start.Zoom, start.X - dx / (fit.Width * start.Zoom), start.Y - dy / (fit.Height * start.Zoom)));
    }

    /// <summary>Once the view settles, develops what is in it at full resolution.</summary>
    private async Task UpdateDetailAsync()
    {
        if (!_imageOpen) return;
        var ticket = _openTicket;
        try
        {
            if ((await _engine.CallAsync("needsDetail")).Json?.GetValue<bool>() != true) return;
            if ((await _engine.CallAsync("hasFullImage")).Json?.GetValue<bool>() == false)
            {
                Status = I18n.T("正在冲洗全尺寸细节…");
                await _engine.CallAsync("developFull");
                if (ticket == _openTicket) Status = I18n.T("已显示全尺寸细节");
            }
            if (ticket != _openTicket) return;
            await _engine.CallAsync("updateDetail");
            RequestRender();
        }
        catch (EngineException exception)
        {
            if (ticket == _openTicket) Status = I18n.T("无法显示细节：{exception}", ("exception", Messages.Friendly(exception)));
        }
    }

    partial void OnClipShadowsChanged(bool value) => SendClipWarnings();
    partial void OnClipHighlightsChanged(bool value) => SendClipWarnings();

    private void SendClipWarnings()
    {
        _ = Fire(() => _engine.CallAsync("setClipWarnings", new { highlights = ClipHighlights, shadows = ClipShadows }));
        RequestRender();
    }

    [RelayCommand]
    public void ToggleClipWarnings()
    {
        var on = !(ClipShadows && ClipHighlights);
        (ClipShadows, ClipHighlights) = (on, on);
    }

    partial void OnCompareOnChanged(bool value) => SendCompare();
    partial void OnCompareSplitChanged(double value) => SendCompare();

    private void SendCompare()
    {
        var split = _comparingAll ? CompareAll : CompareOn ? CompareSplit : 0;
        _ = Fire(() => _engine.CallAsync("setCompare", new { split }));
        RequestRender();
        OverlayChanged?.Invoke();
    }

    /// <summary>While held, the whole frame shows the photo before editing.</summary>
    public void ShowBefore(bool on)
    {
        if (_comparingAll == on) return;
        _comparingAll = on;
        SendCompare();
    }

    // ---- Tools on the stage ----

    [RelayCommand]
    private void PickWhiteBalance() => Picker = Picker == Picker.WhiteBalance ? Picker.None : Picker.WhiteBalance;

    [RelayCommand]
    private void PickPointColour() => Picker = Picker == Picker.Point ? Picker.None : Picker.Point;

    /// <summary>A click on the stage at (px, py) device pixels with a picker active.</summary>
    public async Task PickAtAsync(double px, double py)
    {
        if (Picker == Picker.None || !_imageOpen) return;
        var ticket = _openTicket;
        var args = new { x = px / _viewport.Width, y = py / _viewport.Height, stageWidth = _viewport.Width, stageHeight = _viewport.Height };
        try
        {
            if (Picker == Picker.Point)
            {
                var hue = (await _engine.CallAsync("hueAt", args)).Json?.GetValue<double>() ?? -1;
                if (ticket != _openTicket) return;
                if (hue < 0)
                {
                    ToolHint = I18n.T("这里接近灰色，请点一处有颜色的地方。");
                    return;
                }
                Picker = Picker.None;
                var width = _edit.GetValueOrDefault("pointWidth");
                ApplyEdit(With(("pointHue", hue, 0), ("pointWidth", width > 0 ? width : 30, 0)));
                return;
            }
            var balance = (await _engine.CallAsync("whiteBalanceAt", args)).Json as JsonObject;
            if (ticket != _openTicket) return;
            if (balance is null)
            {
                ToolHint = I18n.T("这里太暗或已过曝，请点一处中性灰。");
                return;
            }
            Picker = Picker.None;
            ApplyEdit(With(("temperature", balance["temperature"]!.GetValue<double>(), 0), ("tint", balance["tint"]!.GetValue<double>(), 0)));
        }
        catch (EngineException exception)
        {
            Status = Messages.Friendly(exception);
        }
    }

    [RelayCommand]
    public void ToggleCrop() => CropEditing = !CropEditing;

    partial void OnCropEditingChanged(bool value)
    {
        if (value)
        {
            Picker = Picker.None;
            Tool = RetouchTool.None;
            if (_view.Zoom > 1) ApplyView(View.Fit);
        }
        _editChanged = true;
        RequestRender();
        UpdateZoomLabel();
        OverlayChanged?.Invoke();
    }

    public double? CropRatio => CropRatios[Math.Clamp(CropRatioIndex, 0, CropRatios.Length - 1)].Ratio;

    partial void OnCropRatioIndexChanged(int value)
    {
        if (CropRatio is { } ratio && _imageOpen)
        {
            var (width, height) = FrameOfImage();
            SetCrop(Frame.CropForRatio(ratio, width, height));
        }
    }

    /// <summary>The crop overlay's geometry in device-independent units: the whole frame on the stage.</summary>
    public (Box ImageBox, Crop Crop, double FrameWidth, double FrameHeight) CropOverlayState()
    {
        var (width, height) = FrameOfImage();
        var box = Frame.FitBox(width / height, _viewport.Width, _viewport.Height);
        return (new Box(box.Left / _scaling, box.Top / _scaling, box.Width / _scaling, box.Height / _scaling), Frame.CropOf(_edit), width, height);
    }

    public void SetCrop(Crop crop)
    {
        var (width, height) = FrameOfImage();
        var next = new Dictionary<string, double>(_edit);
        Frame.SetCrop(next, Frame.ShrinkToImage(crop, _edit.GetValueOrDefault("rotate"), width, height));
        ApplyEdit(next);
    }

    [RelayCommand]
    private void Turn(string direction)
    {
        var next = With(("quarter", _edit.GetValueOrDefault("quarter") + (direction == "left" ? -1 : 1), 0), ("rotate", 0, 0));
        Frame.SetCrop(next, Crop.Full);
        CropRatioIndex = 0;
        ApplyEdit(next, direction == "left" ? I18n.T("向左旋转 90°") : I18n.T("向右旋转 90°"));
    }

    [RelayCommand]
    private void ResetCrop()
    {
        var next = With(("quarter", 0, 0), ("rotate", 0, 0));
        Frame.SetCrop(next, Crop.Full);
        CropRatioIndex = 0;
        ApplyEdit(next, I18n.T("重置裁剪"));
    }

    [RelayCommand]
    private void CancelTools()
    {
        Picker = Picker.None;
        Tool = RetouchTool.None;
        CropEditing = false;
    }

    // ---- Rendering ----

    /// <summary>Runs an engine call whose result is not needed, reporting a failure.</summary>
    private async Task Fire(Func<Task> call)
    {
        try
        {
            await call();
        }
        catch (EngineException exception)
        {
            Status = Messages.Friendly(exception);
            StatusDetail = Messages.Detail(exception);
        }
    }

    /// <summary>Draws the preview; requests made meanwhile collapse into one more frame.</summary>
    private async void RequestRender()
    {
        if (!_imageOpen) return;
        if (_rendering)
        {
            _renderAgain = true;
            return;
        }
        _rendering = true;
        var ticket = _openTicket;
        try
        {
            do
            {
                _renderAgain = false;
                if (_editChanged)
                {
                    // Taken first: an edit the engine rejects must not block every later frame.
                    _editChanged = false;
                    await _engine.CallRawAsync("setAdjustments", Edits.ToEngineJson(_edit, CropEditing, LocalOverlay));
                }
                var (width, height) = _viewport;
                var rgba = (await _engine.CallAsync("render", new { width, height })).Data;
                // A photo opened meanwhile must not show this frame of the last one.
                if (ticket != _openTicket || !_imageOpen) return;
                ShowFrame(rgba, width, height);
            }
            while (_renderAgain && _imageOpen);
        }
        catch (Exception exception)
        {
            if (ticket == _openTicket) Status = I18n.T("预览失败：{exception}", ("exception", Messages.Friendly(exception)));
        }
        finally
        {
            _rendering = false;
            // A new photo's first render may have been folded into this loop's last pass.
            if (_renderAgain && _imageOpen)
            {
                _renderAgain = false;
                RequestRender();
            }
        }
    }

    /// <summary>Two bitmaps take turns, so the one on screen is never written to.</summary>
    private void ShowFrame(byte[] rgba, int width, int height)
    {
        var frame = _spareFrame is { } spare && spare.PixelSize == new PixelSize(width, height)
            ? spare
            : new WriteableBitmap(new PixelSize(width, height), new Vector(96, 96), PixelFormat.Rgba8888, AlphaFormat.Opaque);
        using (var locked = frame.Lock())
        {
            for (var row = 0; row < height; row++)
            {
                System.Runtime.InteropServices.Marshal.Copy(rgba, row * width * 4, locked.Address + row * locked.RowBytes, width * 4);
            }
        }
        _spareFrame = Preview as WriteableBitmap;
        Preview = frame;
    }

    private static Bitmap BitmapFromRgb(byte[] rgb, int width, int height)
    {
        var bitmap = new WriteableBitmap(new PixelSize(width, height), new Vector(96, 96), PixelFormat.Rgba8888, AlphaFormat.Opaque);
        using var locked = bitmap.Lock();
        var row = new byte[width * 4];
        for (var y = 0; y < height; y++)
        {
            for (var x = 0; x < width; x++)
            {
                var from = (y * width + x) * 3;
                row[x * 4] = rgb[from];
                row[x * 4 + 1] = rgb[from + 1];
                row[x * 4 + 2] = rgb[from + 2];
                row[x * 4 + 3] = 255;
            }
            System.Runtime.InteropServices.Marshal.Copy(row, 0, locked.Address + y * locked.RowBytes, row.Length);
        }
        return bitmap;
    }

    // ---- Export ----

    public void SaveOutput(OutputSettings settings)
    {
        Output = settings.Copy();
        _outputFile.Save(Output);
    }

    /// <summary>Exports the open photo at full size; the format follows the file extension.</summary>
    public Task ExportAsync(string path)
    {
        var settings = Output.Copy();
        settings.Format = System.IO.Path.GetExtension(path).ToLowerInvariant() switch
        {
            ".png" => "png",
            ".tif" or ".tiff" => "tiff",
            _ => "jpeg",
        };
        return ExportWithAsync(settings, _ => path);
    }

    /// <summary>Exports the open photo into a folder, named by the settings' template.</summary>
    public Task<string?> ExportToFolderAsync(OutputSettings settings, string folder)
    {
        SaveOutput(settings);
        return ExportWithAsync(settings, extension =>
        {
            var used = Directory.EnumerateFiles(folder).Select(file => System.IO.Path.GetFileName(file).ToLowerInvariant()).ToHashSet();
            return System.IO.Path.Combine(folder, global::OpenPixels.Desktop.Output.ExportName(SelectedPhoto?.Name ?? "photo", used, settings.Template, 1, extension));
        });
    }

    private async Task<string?> ExportWithAsync(OutputSettings settings, Func<string, string> pathFor)
    {
        if (!_imageOpen) return null;
        var ticket = _openTicket;
        Status = I18n.T("导出中…");
        StatusDetail = null;
        try
        {
            var started = Environment.TickCount64;
            var full = await _engine.CallAsync("hasFullImage");
            if (full.Json?.GetValue<bool>() == false)
            {
                Status = I18n.T("正在冲洗全尺寸图像…");
                await _engine.CallAsync("developFull");
            }
            var size = (await _engine.CallAsync("exportSize", new { longEdge = settings.LongEdge })).Json!;
            var overlay = global::OpenPixels.Desktop.Output.RenderWatermark(settings.Watermark, (int)size["width"]!, (int)size["height"]!);
            var args = new
            {
                settings = settings.EngineSettings,
                overlayX = overlay?.X ?? 0,
                overlayY = overlay?.Y ?? 0,
                overlayWidth = overlay?.Width ?? 0,
                overlayHeight = overlay?.Height ?? 0,
            };
            var bytes = (await _engine.CallAsync("exportImage", args, overlay?.Rgba)).Data;
            if (ticket != _openTicket)
            {
                Status = I18n.T("导出已取消：导出时切换了照片");
                return null;
            }
            var path = pathFor(settings.Extension);
            await File.WriteAllBytesAsync(path, bytes);
            Status = I18n.T("已导出 {file}（{size} MB，{ms} ms）", ("file", System.IO.Path.GetFileName(path)), ("size", (bytes.Length / 1048576.0).ToString("0.0")), ("ms", Environment.TickCount64 - started));
            return path;
        }
        catch (Exception exception)
        {
            Status = I18n.T("导出失败：{exception}", ("exception", Messages.Friendly(exception)));
            StatusDetail = Messages.Detail(exception);
            return null;
        }
    }

    public string SuggestedExportName => SelectedPhoto is null ? "export.jpg" : System.IO.Path.GetFileNameWithoutExtension(SelectedPhoto.Name) + ".jpg";

    public void Dispose()
    {
        SaveSession();
        _engine.Dispose();
        if (_ai.IsValueCreated) _ai.Value.Dispose();
        if (_faces.IsValueCreated) _faces.Value.Dispose();
        if (_thumbnailer.IsValueCreated) _thumbnailer.Value.Dispose();
    }
}
