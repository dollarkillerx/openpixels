using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;

namespace OpenPixels.Desktop;

public partial class MainWindow : Window
{
    private static readonly FilePickerFileType Photos = new("照片") { Patterns = PhotoLoader.Extensions.Select(extension => "*" + extension).ToArray() };
    /// <summary>How near, in device-independent pixels, a press must be to grab the compare divider.</summary>
    private const double DividerGrab = 10;
    private const double WheelZoomSpeed = 0.2;

    private readonly MainViewModel _model = new();
    private (Point Start, View View)? _pan;
    private bool _draggingDivider;

    public MainWindow()
    {
        InitializeComponent();
        DataContext = _model;
        Closed += (_, _) => _model.Dispose();
        Curve.Changed += _model.SetCurve;
        CropLayer.Changed += _model.SetCrop;
        _model.OverlayChanged += UpdateOverlays;
        AddHandler(DragDrop.DropEvent, OnDrop);
        AddHandler(DragDrop.DragOverEvent, OnDragOver);
        AddHandler(KeyDownEvent, OnKeyDown, RoutingStrategies.Tunnel);
        AddHandler(KeyUpEvent, OnKeyUp, RoutingStrategies.Tunnel);
    }

    private async void OnOpen(object? sender, RoutedEventArgs e)
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions { Title = "打开照片", AllowMultiple = true, FileTypeFilter = [Photos] });
        _model.AddPhotos(files.Select(file => file.TryGetLocalPath()).OfType<string>());
    }

    private async void OnOpenFolder(object? sender, RoutedEventArgs e)
    {
        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions { Title = "导入文件夹" });
        if (folders.FirstOrDefault()?.TryGetLocalPath() is { } folder) _model.AddFolder(folder);
    }

    private async void OnExport(object? sender, RoutedEventArgs e)
    {
        if (!_model.CanEdit) return;
        var dialog = new ExportWindow(_model.Output);
        await dialog.ShowDialog(this);
        if (dialog.Result is { } result) await _model.ExportToFolderAsync(result.Settings, result.Folder);
    }

    private async void OnImportLook(object? sender, RoutedEventArgs e)
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "导入 LUT",
            FileTypeFilter = [new FilePickerFileType("3D LUT") { Patterns = ["*.cube"] }],
        });
        if (files.FirstOrDefault()?.TryGetLocalPath() is { } path) _model.ImportLook(path);
    }

    private void OnDragOver(object? sender, DragEventArgs e)
    {
        e.DragEffects = e.DataTransfer.TryGetFiles() is { Length: > 0 } ? DragDropEffects.Copy : DragDropEffects.None;
    }

    private void OnDrop(object? sender, DragEventArgs e)
    {
        if (e.DataTransfer.TryGetFiles() is { } items) _model.AddDropped(items.Select(item => item.TryGetLocalPath()).OfType<string>());
    }

    /// <summary>The preview is drawn at the stage's size in device pixels, so it stays sharp.</summary>
    private void OnStageResized(object? sender, SizeChangedEventArgs e)
    {
        var scale = RenderScaling;
        _model.SetViewport((int)(e.NewSize.Width * scale), (int)(e.NewSize.Height * scale), scale);
    }

    private void UpdateOverlays()
    {
        var (box, crop, frameWidth, frameHeight) = _model.CropOverlayState();
        CropLayer.ImageBox = box;
        CropLayer.Crop = crop;
        CropLayer.Ratio = _model.CropRatio;
        CropLayer.FrameWidth = frameWidth;
        CropLayer.FrameHeight = frameHeight;
        CropLayer.InvalidateVisual();

        var (width, height) = (Stage.Bounds.Width, Stage.Bounds.Height);
        var x = _model.CompareSplit * width;
        Divider.Height = height;
        Canvas.SetLeft(Divider, x - 1);
        Canvas.SetLeft(BeforeLabel, Math.Max(4, x - 52));
        Canvas.SetTop(BeforeLabel, 8);
        Canvas.SetLeft(AfterLabel, Math.Min(width - 50, x + 6));
        Canvas.SetTop(AfterLabel, 8);
    }

    private Point DevicePoint(Point point) => new(point.X * RenderScaling, point.Y * RenderScaling);

    private async void OnStagePressed(object? sender, PointerPressedEventArgs e)
    {
        if (!_model.CanEdit || e.Handled || !e.GetCurrentPoint(Stage).Properties.IsLeftButtonPressed) return;
        var position = e.GetPosition(Stage);
        if (_model.Picker != Picker.None)
        {
            var device = DevicePoint(position);
            await _model.PickAtAsync(device.X, device.Y);
            return;
        }
        if (_model.CropEditing) return;
        if (_model.CompareOn && Math.Abs(position.X - _model.CompareSplit * Stage.Bounds.Width) <= DividerGrab)
        {
            _draggingDivider = true;
            e.Pointer.Capture(Stage);
            return;
        }
        if (_model.IsZoomed)
        {
            _pan = (position, _model.CurrentView);
            e.Pointer.Capture(Stage);
            Stage.Cursor = new Cursor(StandardCursorType.SizeAll);
        }
    }

    private void OnStageMoved(object? sender, PointerEventArgs e)
    {
        var position = e.GetPosition(Stage);
        if (_draggingDivider)
        {
            _model.CompareSplit = Math.Clamp(position.X / Math.Max(1, Stage.Bounds.Width), 0, 1);
            return;
        }
        if (_pan is { } pan) _model.PanFrom(pan.View, (position.X - pan.Start.X) * RenderScaling, (position.Y - pan.Start.Y) * RenderScaling);
    }

    private void OnStageReleased(object? sender, PointerReleasedEventArgs e)
    {
        _pan = null;
        _draggingDivider = false;
        Stage.Cursor = null;
        e.Pointer.Capture(null);
    }

    private void OnStageWheel(object? sender, PointerWheelEventArgs e)
    {
        if (!_model.CanEdit) return;
        var device = DevicePoint(e.GetPosition(Stage));
        _model.ZoomBy(Math.Exp(e.Delta.Y * WheelZoomSpeed), device.X, device.Y);
        e.Handled = true;
    }

    private void OnStageDoubleTapped(object? sender, TappedEventArgs e)
    {
        if (!_model.CanEdit || _model.Picker != Picker.None || _model.CropEditing) return;
        if (_model.IsZoomed)
        {
            _model.FitViewCommand.Execute(null);
            return;
        }
        var device = DevicePoint(e.GetPosition(Stage));
        _model.ZoomAround(double.MaxValue, device.X, device.Y);
    }

    private void OnSliderDoubleTapped(object? sender, TappedEventArgs e)
    {
        if (sender is Slider { DataContext: SliderItem slider }) slider.ResetCommand.Execute(null);
    }

    private void OnWheelPicked(object? sender, WheelPickedEventArgs e)
    {
        if (sender is ColorWheel { DataContext: GradeItem grade }) _model.SetGrade(grade, e.Hue, e.Amount);
    }

    private void OnKeyDown(object? sender, KeyEventArgs e)
    {
        // Typing in a text box keeps its own keys.
        if (FocusManager?.GetFocusedElement() is TextBox) return;
        var command = e.KeyModifiers.HasFlag(KeyModifiers.Meta) || e.KeyModifiers.HasFlag(KeyModifiers.Control);
        var shift = e.KeyModifiers.HasFlag(KeyModifiers.Shift);
        switch (e.Key)
        {
            case Key.Z when command && shift:
            case Key.Y when command:
                _model.Redo();
                break;
            case Key.Z when command:
                _model.Undo();
                break;
            case Key.O when command:
                OnOpen(this, e);
                break;
            case Key.E when command:
                OnExport(this, e);
                break;
            case Key.Z when !command:
                _model.ToggleZoom();
                break;
            case Key.J when !command:
                _model.ToggleClipWarnings();
                break;
            case Key.R when !command:
                if (_model.CanEdit) _model.ToggleCrop();
                break;
            case Key.W when !command:
                if (_model.CanEdit) _model.PickWhiteBalanceCommand.Execute(null);
                break;
            case Key.Escape:
                _model.CancelToolsCommand.Execute(null);
                break;
            case Key.Left when !command && FocusManager?.GetFocusedElement() is not Slider:
                _model.SelectNext(-1);
                break;
            case Key.Right when !command && FocusManager?.GetFocusedElement() is not Slider:
                _model.SelectNext(1);
                break;
            default:
                if (e.KeySymbol == "\\" && _model.CanEdit)
                {
                    _model.ShowBefore(true);
                    break;
                }
                return;
        }
        e.Handled = true;
    }

    private void OnKeyUp(object? sender, KeyEventArgs e)
    {
        if (e.KeySymbol == "\\") _model.ShowBefore(false);
    }
}
