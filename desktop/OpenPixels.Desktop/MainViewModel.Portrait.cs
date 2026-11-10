using System.Diagnostics;
using CommunityToolkit.Mvvm.ComponentModel;
using OpenPixels.Desktop.Engine;

namespace OpenPixels.Desktop;

/// <summary>Portrait retouching: faces found once per photo give the skin to smooth and clear.</summary>
public sealed partial class MainViewModel
{
    /// <summary>Faces are looked for at this size, then in a larger copy if none is found, as in the web edition.</summary>
    private const int FaceLongEdge = 1024;
    private const int SmallFaceLongEdge = 2048;

    private sealed record FoundFaces(List<FaceMesh> Faces, long Milliseconds);

    /// <summary>Faces per photo, as fractions of the frame, so any engine at any size can use them.</summary>
    private readonly Dictionary<string, FoundFaces> _faceCache = [];
    private readonly Lock _faceLock = new();

    public SliderGroup SkinGroup { get; private set; } = null!;

    [ObservableProperty]
    private string _skinStatus = "";

    private void MakePortraitSliders() => SkinGroup = new SliderGroup(Edits.Skin.Title, Edits.Skin.Sliders.Select(Make).ToList());

    public static bool NeedsSkin(IReadOnlyDictionary<string, double> edit) => edit.GetValueOrDefault("skinSmooth") != 0 || edit.GetValueOrDefault("skinBlemish") != 0;

    /// <summary>Faces in a culling thumbnail; none when the face models are missing or fail.</summary>
    private List<FaceMesh> FindFaces(byte[] rgb, int width, int height)
    {
        if (!FaceFinder.Available) return [];
        try
        {
            return _faces.Value.Find(rgb, width, height);
        }
        catch (Exception exception) when (exception is not OutOfMemoryException)
        {
            return [];
        }
    }

    /// <summary>The photo's faces, found with the given engine, which must have it open, the first time.</summary>
    private async Task<FoundFaces> FacesAsync(EngineClient engine, string id)
    {
        lock (_faceLock)
        {
            if (_faceCache.TryGetValue(id, out var known)) return known;
        }
        var clock = Stopwatch.StartNew();
        var faces = new List<FaceMesh>();
        if (FaceFinder.Available)
        {
            async Task<(byte[] Rgb, int Width, int Height)> Thumbnail(int longEdge)
            {
                var size = (await engine.CallAsync("thumbnailSize", new { maxLongEdge = longEdge })).Json!;
                var rgb = (await engine.CallAsync("thumbnail", new { maxLongEdge = longEdge })).Data;
                return (rgb, (int)size["width"]!, (int)size["height"]!);
            }
            var (rgb, width, height) = await Thumbnail(FaceLongEdge);
            // The larger copy is only made if the first look finds nothing whole.
            faces = await Task.Run(() => _faces.Value.Find(rgb, width, height, () => Thumbnail(SmallFaceLongEdge).GetAwaiter().GetResult()));
        }
        var found = new FoundFaces(faces, clock.ElapsedMilliseconds);
        lock (_faceLock) _faceCache[id] = found;
        return found;
    }

    /// <summary>Sends the skin mask an edit with smoothing or blemish removal needs.</summary>
    private async Task ApplySkinAsync(EngineClient engine, string id, IReadOnlyDictionary<string, double> edit, (int Width, int Height) proxy)
    {
        if (!NeedsSkin(edit)) return;
        var found = await FacesAsync(engine, id);
        var (mask, faceWidth) = await Task.Run(() => FaceRegions.SkinMask(found.Faces, proxy.Width, proxy.Height));
        await engine.CallAsync("setSkinMask", new { faceWidth }, mask);
    }

    private void ShowSkinStatus(string id, IReadOnlyDictionary<string, double> edit)
    {
        FoundFaces? found;
        lock (_faceLock) _faceCache.TryGetValue(id, out found);
        SkinStatus = found is null || !NeedsSkin(edit) ? ""
            : found.Faces.Count > 0 ? I18n.T("检测到 {count} 张人脸，{ms} ms", ("count", found.Faces.Count), ("ms", found.Milliseconds))
            : I18n.T("未检测到完整人脸，按画面中的肤色处理");
    }

    /// <summary>Finds faces the first time smoothing or blemish removal is turned on.</summary>
    private async Task UpdateSkinAsync()
    {
        if (SelectedPhoto is not { } photo || !_imageOpen) return;
        var ticket = _openTicket;
        AiBusy = true;
        SkinStatus = I18n.T("识别中…");
        try
        {
            await ApplySkinAsync(_engine, photo.Id, _edit, _proxySize);
            if (ticket != _openTicket) return;
            ShowSkinStatus(photo.Id, _edit);
            RequestRender();
            _thumbnailTimer.Stop();
            _thumbnailTimer.Start();
        }
        catch (Exception exception)
        {
            SkinStatus = I18n.T("人脸识别失败：{message}", ("message", Messages.Friendly(exception)));
        }
        finally
        {
            AiBusy = false;
        }
    }
}
