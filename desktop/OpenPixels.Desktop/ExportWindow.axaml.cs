using System.Collections.ObjectModel;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace OpenPixels.Desktop;

/// <summary>Export settings being edited, plus named presets of them.</summary>
public sealed partial class ExportViewModel : ObservableObject
{
    private readonly JsonFile<Dictionary<string, OutputSettings>> _presetFile = new("outputPresets.json");
    private readonly Dictionary<string, OutputSettings> _presets;

    private readonly List<ExportScope> _scopes = [ExportScope.Current];

    public ExportViewModel(OutputSettings settings, int selected = 1, int visible = 1)
    {
        Settings = settings.Copy();
        _presets = _presetFile.Load();
        PresetNames = new(_presets.Keys.Order());
        var scopes = new List<string> { I18n.T("当前照片") };
        if (selected > 1)
        {
            scopes.Add(I18n.T("选中的 {selected} 张", ("selected", selected)));
            _scopes.Add(ExportScope.Selected);
        }
        if (visible > 1)
        {
            scopes.Add(I18n.T("列表中的全部 {visible} 张", ("visible", visible)));
            _scopes.Add(ExportScope.Visible);
        }
        Scopes = [.. scopes];
    }

    public string[] Scopes { get; }

    [ObservableProperty]
    private int _scopeIndex;

    public ExportScope Scope => _scopes[Math.Clamp(ScopeIndex, 0, _scopes.Count - 1)];
    public bool HasChoiceOfPhotos => Scopes.Length > 1;

    public OutputSettings Settings { get; private set; }

    public string[] Formats { get; } = Output.Formats.Select(format => format.Label).ToArray();
    public string[] ColorSpaces { get; } = Output.ColorSpaces.Select(space => space.Label).ToArray();
    public string[] BitDepths { get; } = [I18n.T("8 位"), I18n.T("16 位")];
    public string[] LongEdges { get; } = Output.LongEdges.Select(edge => edge == 0 ? I18n.T("原始尺寸") : I18n.T("长边 {edge} 像素", ("edge", edge))).ToArray();
    public string[] Positions { get; } = Output.Positions.Select(position => position.Label).ToArray();
    public string TemplateHint => I18n.T("可用 {name} 原文件名、{n} 序号、{date} 日期");
    public ObservableCollection<string> PresetNames { get; }

    [ObservableProperty]
    private string? _selectedPreset;

    [ObservableProperty]
    private string _newPresetName = "";

    partial void OnSelectedPresetChanged(string? value)
    {
        if (value is not null && _presets.TryGetValue(value, out var preset)) Load(preset);
    }

    private void Load(OutputSettings settings)
    {
        Settings = settings.Copy();
        OnPropertyChanged(string.Empty);
    }

    [RelayCommand]
    private void SavePreset()
    {
        var name = NewPresetName.Trim();
        if (name.Length == 0) return;
        _presets[name] = Settings.Copy();
        _presetFile.Save(_presets);
        if (!PresetNames.Contains(name)) PresetNames.Add(name);
        NewPresetName = "";
        SelectedPreset = name;
    }

    [RelayCommand]
    private void DeletePreset()
    {
        if (SelectedPreset is not { } name) return;
        _presets.Remove(name);
        _presetFile.Save(_presets);
        PresetNames.Remove(name);
        SelectedPreset = null;
    }

    private static int IndexOf((string Value, string Label)[] choices, string value) => Math.Max(0, Array.FindIndex(choices, choice => choice.Value == value));

    public int FormatIndex
    {
        get => IndexOf(Output.Formats, Settings.Format);
        set
        {
            Settings.Format = Output.Formats[Math.Clamp(value, 0, Output.Formats.Length - 1)].Value;
            OnPropertyChanged();
            OnPropertyChanged(nameof(IsJpeg));
        }
    }

    public bool IsJpeg => Settings.Format == "jpeg";

    public int Quality
    {
        get => Settings.Quality;
        set => SetProperty(Settings.Quality, Math.Clamp(value, 1, 100), Settings, (s, v) => s.Quality = v);
    }

    public int BitDepthIndex
    {
        get => Settings.BitDepth == 16 ? 1 : 0;
        set => SetProperty(BitDepthIndex, value, Settings, (s, v) => s.BitDepth = v == 1 ? 16 : 8);
    }

    public int ColorSpaceIndex
    {
        get => IndexOf(Output.ColorSpaces, Settings.ColorSpace);
        set => SetProperty(ColorSpaceIndex, value, Settings, (s, v) => s.ColorSpace = Output.ColorSpaces[Math.Clamp(v, 0, Output.ColorSpaces.Length - 1)].Value);
    }

    public int LongEdgeIndex
    {
        get => Math.Max(0, Array.IndexOf(Output.LongEdges, Settings.LongEdge));
        set => SetProperty(LongEdgeIndex, value, Settings, (s, v) => s.LongEdge = Output.LongEdges[Math.Clamp(v, 0, Output.LongEdges.Length - 1)]);
    }

    public string Template
    {
        get => Settings.Template;
        set => SetProperty(Settings.Template, value, Settings, (s, v) => s.Template = v);
    }

    public bool WatermarkEnabled
    {
        get => Settings.Watermark.Enabled;
        set => SetProperty(Settings.Watermark.Enabled, value, Settings.Watermark, (w, v) => w.Enabled = v);
    }

    public string WatermarkText
    {
        get => Settings.Watermark.Text;
        set => SetProperty(Settings.Watermark.Text, value, Settings.Watermark, (w, v) => w.Text = v);
    }

    public double WatermarkSize
    {
        get => Settings.Watermark.Size;
        set => SetProperty(Settings.Watermark.Size, value, Settings.Watermark, (w, v) => w.Size = v);
    }

    public double WatermarkOpacity
    {
        get => Settings.Watermark.Opacity;
        set => SetProperty(Settings.Watermark.Opacity, value, Settings.Watermark, (w, v) => w.Opacity = v);
    }

    public int PositionIndex
    {
        get => IndexOf(Output.Positions, Settings.Watermark.Position);
        set => SetProperty(PositionIndex, value, Settings.Watermark, (w, v) => w.Position = Output.Positions[Math.Clamp(v, 0, Output.Positions.Length - 1)].Value);
    }

    public bool HasLogo => Settings.Watermark.LogoPath is { Length: > 0 };
    public string LogoName => HasLogo ? Path.GetFileName(Settings.Watermark.LogoPath!) : "";

    public void SetLogo(string? path)
    {
        Settings.Watermark.LogoPath = path;
        OnPropertyChanged(nameof(HasLogo));
        OnPropertyChanged(nameof(LogoName));
    }

    [RelayCommand]
    private void ClearLogo() => SetLogo(null);
}

public enum ExportScope { Current, Selected, Visible }

public partial class ExportWindow : Window
{
    private readonly ExportViewModel _model;

    public ExportWindow() : this(new OutputSettings(), 1, 1)
    {
    }

    public ExportWindow(OutputSettings settings, int selected, int visible)
    {
        InitializeComponent();
        _model = new ExportViewModel(settings, selected, visible);
        DataContext = _model;
    }

    /// <summary>The settings, the folder and which photos, or null if cancelled.</summary>
    public (OutputSettings Settings, string Folder, ExportScope Scope)? Result { get; private set; }

    private void OnCancel(object? sender, RoutedEventArgs e) => Close();

    private async void OnPickLogo(object? sender, RoutedEventArgs e)
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = I18n.T("选择水印图片"),
            FileTypeFilter = [new FilePickerFileType(I18n.T("图片")) { Patterns = ["*.png", "*.jpg", "*.jpeg"] }],
        });
        if (files.FirstOrDefault()?.TryGetLocalPath() is { } path) _model.SetLogo(path);
    }

    private async void OnExport(object? sender, RoutedEventArgs e)
    {
        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions { Title = I18n.T("导出到文件夹") });
        if (folders.FirstOrDefault()?.TryGetLocalPath() is not { } folder) return;
        Result = (_model.Settings, folder, _model.Scope);
        Close();
    }
}
