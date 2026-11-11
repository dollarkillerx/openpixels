using Microsoft.ML.OnnxRuntime;
using Microsoft.ML.OnnxRuntime.Tensors;
using Microsoft.ML.Tokenizers;

namespace OpenPixels.Desktop;

/// <summary>A tag the library can give a photo by looking at it.</summary>
/// <param name="Key">Stable name, used in saved filters.</param>
/// <param name="Prompt">What SigLIP is asked, in English, which it reads best.</param>
/// <param name="Threshold">The similarity above which the tag applies, found on sample photos.</param>
/// <param name="Names">The tag's name in each interface language.</param>
public sealed record PhotoTag(string Key, string Prompt, double Threshold, IReadOnlyDictionary<string, string> Names)
{
    public string Name => Names.TryGetValue(I18n.Language, out var name) ? name : Names["en"];
}

/// <summary>
/// Search by description and automatic tags, with SigLIP 2 (Apache-2.0): photos and words are
/// mapped into one space, so "海边的日落" or "flowers" finds the photos that look like it in any
/// of the interface languages. A photo is embedded once, from the thumbnail the library analysis
/// already makes; a query takes a few tens of milliseconds.
/// </summary>
public sealed class ImageSearch : IDisposable
{
    public const string VisionFile = "siglip/vision_model.onnx";
    public const string TextFile = "siglip/text_model.onnx";
    public const string TokenizerFile = "siglip/tokenizer.model";
    public const int Size = 224;
    public const int Dimensions = 768;
    private const int TextLength = 64;
    private const long PadId = 0;
    private const long EosId = 1;
    /// <summary>Below this a photo does not match a query; matches on sample photos were 0.07 to 0.13.</summary>
    public const double MatchThreshold = 0.065;
    private const int MaxTags = 3;

    public static bool Available => AiModels.Has(VisionFile) && AiModels.Has(TextFile) && AiModels.Has(TokenizerFile);

    private static PhotoTag Tag(string key, string prompt, double threshold, string zh, string tw, string en, string ja, string ko) =>
        new(key, prompt, threshold, new Dictionary<string, string> { ["zh-CN"] = zh, ["zh-TW"] = tw, ["en"] = en, ["ja"] = ja, ["ko"] = ko });

    /// <summary>Tags that SigLIP tells apart reliably. Prompts that matched every photo, such as close-ups, were left out.</summary>
    public static readonly PhotoTag[] Tags =
    [
        Tag("person", "a photo of a person", 0.085, "人物", "人物", "People", "人物", "인물"),
        Tag("portrait", "a portrait photo of a face", 0.09, "人像", "人像", "Portrait", "ポートレート", "인물 사진"),
        Tag("group", "a photo of a group of people", 0.08, "合影", "合照", "Group", "集合写真", "단체 사진"),
        Tag("landscape", "a photo of a landscape", 0.085, "风景", "風景", "Landscape", "風景", "풍경"),
        Tag("mountain", "a photo of mountains", 0.075, "山", "山", "Mountains", "山", "산"),
        Tag("water", "a photo of a lake", 0.08, "湖泊", "湖泊", "Lake", "湖", "호수"),
        Tag("sea", "a photo of the sea", 0.08, "海", "海", "Sea", "海", "바다"),
        Tag("sunset", "a photo of a sunset", 0.085, "日落", "日落", "Sunset", "夕焼け", "일몰"),
        Tag("night", "a photo taken at night", 0.09, "夜景", "夜景", "Night", "夜景", "야경"),
        Tag("street", "a photo of a city street", 0.085, "街景", "街景", "Street", "街並み", "거리"),
        Tag("building", "a photo of a building", 0.09, "建筑", "建築", "Architecture", "建築", "건축"),
        Tag("flower", "a photo of flowers", 0.085, "花", "花", "Flowers", "花", "꽃"),
        Tag("tree", "a photo of trees", 0.08, "树木", "樹木", "Trees", "木", "나무"),
        Tag("sky", "a photo of the sky", 0.08, "天空", "天空", "Sky", "空", "하늘"),
        Tag("snow", "a photo of snow", 0.068, "雪", "雪", "Snow", "雪", "눈"),
        Tag("animal", "a photo of an animal", 0.08, "动物", "動物", "Animals", "動物", "동물"),
        Tag("food", "a photo of food", 0.08, "美食", "美食", "Food", "料理", "음식"),
        Tag("car", "a photo of a car", 0.08, "汽车", "汽車", "Cars", "車", "자동차"),
    ];

    private readonly Lazy<InferenceSession> _vision = new(() => Open(VisionFile));
    private readonly Lazy<InferenceSession> _text = new(() => Open(TextFile));
    private readonly Lazy<Tokenizer> _tokenizer = new(() =>
    {
        using var stream = File.OpenRead(Path.Combine(AiModels.Folder, TokenizerFile));
        return SentencePieceTokenizer.Create(stream, addBeginningOfSentence: false, addEndOfSentence: true);
    });
    private readonly Dictionary<string, float[]> _texts = [];
    private readonly Lock _lock = new();

    private static InferenceSession Open(string file)
    {
        var path = Path.Combine(AiModels.Folder, file);
        if (!File.Exists(path)) throw new Engine.UserMessageException(I18n.T("模型文件不存在，请先运行 scripts/fetch-model.sh"));
        using var options = new SessionOptions { GraphOptimizationLevel = GraphOptimizationLevel.ORT_ENABLE_ALL };
        return new InferenceSession(path, options);
    }

    private static float[] Normalized(float[] values)
    {
        var length = MathF.Sqrt(values.Sum(value => value * value));
        return length > 0 ? values.Select(value => value / length).ToArray() : values;
    }

    /// <summary>The model's input from interleaved sRGB bytes: the whole frame squeezed to a square, as SigLIP was trained.</summary>
    public static float[] Input(byte[] rgb, int width, int height)
    {
        const int size = Size;
        var planes = new float[3 * size * size];
        for (var y = 0; y < size; y++)
        {
            // Each output pixel averages the block of source pixels it covers.
            var (top, bottom) = (y * height / size, Math.Max(y * height / size + 1, (y + 1) * height / size));
            for (var x = 0; x < size; x++)
            {
                var (left, right) = (x * width / size, Math.Max(x * width / size + 1, (x + 1) * width / size));
                double r = 0, g = 0, b = 0;
                for (var sy = top; sy < bottom; sy++)
                {
                    for (var sx = left; sx < right; sx++)
                    {
                        var at = (sy * width + sx) * 3;
                        (r, g, b) = (r + rgb[at], g + rgb[at + 1], b + rgb[at + 2]);
                    }
                }
                var count = (bottom - top) * (right - left);
                // Scaled to -1..1: mean and spread of 0.5.
                planes[y * size + x] = (float)(r / count / 127.5 - 1);
                planes[size * size + y * size + x] = (float)(g / count / 127.5 - 1);
                planes[2 * size * size + y * size + x] = (float)(b / count / 127.5 - 1);
            }
        }
        return planes;
    }

    /// <summary>A photo's place in the shared space, of unit length.</summary>
    public float[] EmbedImage(byte[] rgb, int width, int height)
    {
        var input = NamedOnnxValue.CreateFromTensor("pixel_values", new DenseTensor<float>(Input(rgb, width, height), [1, 3, Size, Size]));
        using var results = _vision.Value.Run([input]);
        return Normalized(results.First(result => result.Name == "pooler_output").AsTensor<float>().ToArray());
    }

    /// <summary>A description's place in the shared space, of unit length. Lower case and padded to 64 tokens, as SigLIP was trained.</summary>
    public float[] EmbedText(string text)
    {
        var key = text.Trim().ToLowerInvariant();
        lock (_lock)
        {
            if (_texts.TryGetValue(key, out var known)) return known;
        }
        var ids = _tokenizer.Value.EncodeToIds(key).Select(id => (long)id).ToList();
        if (ids.Count > TextLength) ids = [.. ids.Take(TextLength - 1), EosId];
        while (ids.Count < TextLength) ids.Add(PadId);
        var input = NamedOnnxValue.CreateFromTensor("input_ids", new DenseTensor<long>(ids.ToArray(), [1, TextLength]));
        using var results = _text.Value.Run([input]);
        var embedding = Normalized(results.First(result => result.Name == "pooler_output").AsTensor<float>().ToArray());
        lock (_lock) _texts[key] = embedding;
        return embedding;
    }

    public static double Similarity(float[] a, float[] b)
    {
        double total = 0;
        for (var i = 0; i < Math.Min(a.Length, b.Length); i++) total += a[i] * b[i];
        return total;
    }

    /// <summary>The tags a photo shows most clearly, at most three, strongest first.</summary>
    public IReadOnlyList<PhotoTag> TagsOf(float[] image) =>
        Tags.Select(tag => (Tag: tag, Margin: Similarity(image, EmbedText(tag.Prompt)) - tag.Threshold))
            .Where(scored => scored.Margin > 0)
            .OrderByDescending(scored => scored.Margin)
            .Take(MaxTags)
            .Select(scored => scored.Tag)
            .ToList();

    /// <summary>An embedding stored compactly, as half floats in base64.</summary>
    public static string Pack(float[] embedding) =>
        Convert.ToBase64String(embedding.SelectMany(value => BitConverter.GetBytes((Half)value)).ToArray());

    public static float[]? Unpack(string packed)
    {
        try
        {
            var bytes = Convert.FromBase64String(packed);
            if (bytes.Length != Dimensions * 2) return null;
            return Enumerable.Range(0, Dimensions).Select(i => (float)BitConverter.ToHalf(bytes, i * 2)).ToArray();
        }
        catch (FormatException)
        {
            return null;
        }
    }

    public void Dispose()
    {
        if (_vision.IsValueCreated) _vision.Value.Dispose();
        if (_text.IsValueCreated) _text.Value.Dispose();
    }
}
