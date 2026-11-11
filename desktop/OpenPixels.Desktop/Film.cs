using System.Text.Json;

namespace OpenPixels.Desktop;

/// <summary>A print stock a negative can be printed on.</summary>
public sealed record FilmPrint(int Id, string Kind, string Name)
{
    public override string ToString() => Name;
}

/// <summary>
/// The desktop's film simulation: for each film look, a negative LUT that can be paired with
/// any print (or, for a slide, its own projection), made by scripts/generate-film-looks.py
/// from the film data in spectral_film_lut (MIT) and installed in film-sim/.
/// </summary>
public static class FilmSim
{
    public sealed record FilmInfo(int Id, string Kind, bool Mono, int Print, List<double[]> Grain)
    {
        public bool Slide => Kind == "slide";
    }

    private sealed record Settings(int Size, List<FilmPrint> Prints, List<FilmInfo> Films);

    /// <summary>Frame long edges in millimetres for 16mm, 35mm and 70mm film.</summary>
    public static readonly double[] FormatsMm = [12.5, 36, 70];
    public static readonly string[] FormatNames = ["16mm", "35mm", "70mm"];
    public const int DefaultFormat = 1;

    public static string Folder => Path.Combine(AppContext.BaseDirectory, "film-sim");

    private static readonly Lazy<Settings?> Loaded = new(() =>
    {
        try
        {
            using var stream = File.OpenRead(Path.Combine(Folder, "film-sim.json"));
            return JsonSerializer.Deserialize<Settings>(stream, new JsonSerializerOptions { PropertyNameCaseInsensitive = true });
        }
        catch (Exception exception) when (exception is IOException or JsonException or UnauthorizedAccessException)
        {
            return null;
        }
    });

    private static readonly Dictionary<string, float[]> Cache = [];

    public static bool Available => Loaded.Value is not null;

    public static int Size => Loaded.Value?.Size ?? 0;

    public static IReadOnlyList<FilmPrint> Prints => Loaded.Value?.Prints ?? [];

    /// <summary>The film behind a look id, or null if it is not a film look or the files are missing.</summary>
    public static FilmInfo? Film(int lookId) => Loaded.Value?.Films.FirstOrDefault(film => film.Id == lookId);

    public static FilmPrint? Print(int id) => Prints.FirstOrDefault(print => print.Id == id);

    /// <summary>The print a film is shown on in an edit: the one chosen, else its own default.</summary>
    public static int PrintFor(FilmInfo film, IReadOnlyDictionary<string, double> edit)
    {
        var chosen = (int)edit.GetValueOrDefault("filmPrint");
        return Print(chosen) is not null ? chosen : film.Print;
    }

    /// <summary>A LUT file's values in 0..1, or null if it is missing or damaged.</summary>
    public static float[]? Lut(string file)
    {
        lock (Cache)
        {
            if (Cache.TryGetValue(file, out var cached)) return cached;
            byte[] bytes;
            try
            {
                bytes = File.ReadAllBytes(Path.Combine(Folder, file));
            }
            catch (IOException)
            {
                return null;
            }
            var values = new float[Size * Size * Size * 3];
            if (bytes.Length != values.Length * 2) return null;
            for (var i = 0; i < values.Length; i++) values[i] = BitConverter.ToUInt16(bytes, i * 2) / 65535f;
            return Cache[file] = values;
        }
    }

    /// <summary>The setFilm call for a film look in an edit, or null if its files are missing.</summary>
    public static (object Args, byte[] Data)? Call(FilmInfo film, IReadOnlyDictionary<string, double> edit)
    {
        var negative = Lut($"negative-{film.Id}.lut");
        var output = film.Slide ? Lut($"slide-{film.Id}.lut") : Lut($"print-{PrintFor(film, edit)}.lut");
        if (negative is null || output is null) return null;
        var data = new byte[(negative.Length + output.Length) * 4];
        Buffer.BlockCopy(negative, 0, data, 0, negative.Length * 4);
        Buffer.BlockCopy(output, 0, data, negative.Length * 4, output.Length * 4);
        var grain = film.Grain.SelectMany(row => row).ToArray();
        return (new { negativeSize = Size, outputSize = Size, slide = film.Slide, mono = film.Mono, grain }, data);
    }

    /// <summary>The format index stored in an edit as the frame's long edge.</summary>
    public static int FormatIndex(IReadOnlyDictionary<string, double> edit)
    {
        var mm = edit.GetValueOrDefault("filmFormat");
        var index = Array.FindIndex(FormatsMm, value => Math.Abs(value - mm) < 0.01);
        return index >= 0 ? index : DefaultFormat;
    }
}
