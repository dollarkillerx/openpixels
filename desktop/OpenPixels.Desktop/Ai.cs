using Microsoft.ML.OnnxRuntime;
using Microsoft.ML.OnnxRuntime.Tensors;

namespace OpenPixels.Desktop;

/// <summary>
/// The AI models, run with ONNX Runtime on the CPU. Preprocessing matches web/src/model.ts,
/// and post-processing matches web/src/ai-worker.ts and inpaint-worker.ts.
/// </summary>
public sealed class AiModels : IDisposable
{
    /// <summary>MODNet (Apache-2.0): a subject matte.</summary>
    public static class Matte
    {
        public const string File = "modnet.onnx";
        public const int ShortestEdge = 512;
        public const int DivisibleBy = 32;
        public static readonly float[] Mean = [0.5f, 0.5f, 0.5f];
        public static readonly float[] Std = [0.5f, 0.5f, 0.5f];

        /// <summary>Scales so the shortest edge matches the model, keeping aspect ratio.</summary>
        public static (int Width, int Height) InputSize(int width, int height)
        {
            var scale = ShortestEdge / (double)Math.Min(width, height);
            int Fit(int edge) => Math.Max(DivisibleBy, (int)Math.Round(edge * scale / DivisibleBy, MidpointRounding.AwayFromZero) * DivisibleBy);
            return (Fit(width), Fit(height));
        }
    }

    /// <summary>SkyWater-Seg (MIT), which labels background, sky, water and person.</summary>
    public static class Sky
    {
        public const string File = "skywater.onnx";
        // Above the 384 it was trained at, for finer edges around branches and rooftops.
        public const int Size = 640;
        public static readonly float[] Mean = [0.485f, 0.456f, 0.406f];
        public static readonly float[] Std = [0.229f, 0.224f, 0.225f];
        public const int Classes = 4;
        public const int SkyClass = 1;
    }

    /// <summary>LaMa inpainting (Apache-2.0). This export only accepts one input size.</summary>
    public static class Inpaint
    {
        public const string File = "lama.onnx";
        public const int Size = Erasing.ModelSize;
    }

    private readonly Dictionary<string, InferenceSession> _sessions = [];
    private readonly Lock _lock = new();

    public static string Folder => Path.Combine(AppContext.BaseDirectory, "models");

    public static bool Has(string file) => System.IO.File.Exists(Path.Combine(Folder, file));

    private InferenceSession Session(string file)
    {
        lock (_lock)
        {
            if (_sessions.TryGetValue(file, out var session)) return session;
            var path = Path.Combine(Folder, file);
            if (!System.IO.File.Exists(path)) throw new Engine.UserMessageException(I18n.T("模型文件不存在，请先运行 scripts/fetch-model.sh"));
            using var options = new SessionOptions { GraphOptimizationLevel = GraphOptimizationLevel.ORT_ENABLE_ALL };
            return _sessions[file] = new InferenceSession(path, options);
        }
    }

    /// <summary>Runs one model on a planar float tensor and returns its first output.</summary>
    private float[] Run(string file, params (string? Name, float[] Data, int[] Shape)[] inputs)
    {
        var session = Session(file);
        var values = inputs.Select((input, index) =>
            NamedOnnxValue.CreateFromTensor(input.Name ?? session.InputNames[index], new DenseTensor<float>(input.Data, input.Shape))).ToList();
        using var results = session.Run(values);
        return results.First().AsTensor<float>().ToArray();
    }

    /// <summary>The subject matte for a model input from the engine's modelInput, as 0..255.</summary>
    public byte[] SubjectMatte(float[] tensor, int width, int height) =>
        Run(Matte.File, (null, tensor, [1, 3, height, width])).Select(alpha => (byte)Math.Clamp(Math.Round(alpha * 255), 0, 255)).ToArray();

    /// <summary>The sky's probability at each pixel, with stray patches below the sky removed.</summary>
    public byte[] SkyMask(float[] tensor)
    {
        const int size = Sky.Size;
        var logits = Run(Sky.File, (null, tensor, [1, 3, size, size]));
        var pixels = size * size;
        var mask = new byte[pixels];
        for (var i = 0; i < pixels; i++)
        {
            double total = 0;
            for (var c = 0; c < Sky.Classes; c++) total += Math.Exp(logits[c * pixels + i]);
            mask[i] = (byte)Math.Clamp(Math.Round(Math.Exp(logits[Sky.SkyClass * pixels + i]) / total * 255), 0, 255);
        }
        RemoveStraySky(mask, size, size);
        return mask;
    }

    /// <summary>
    /// Clears patches of "sky" entirely below the sky that reaches the top of the frame,
    /// which are really snow, water or bright walls. Gaps between branches are level with
    /// the sky, so they stay.
    /// </summary>
    public static void RemoveStraySky(byte[] mask, int width, int height)
    {
        var labels = new int[mask.Length];
        var regions = new List<(int Top, int Bottom, bool TouchesTop, int Size)>();
        var stack = new Stack<int>();
        for (var start = 0; start < mask.Length; start++)
        {
            if (labels[start] != 0 || mask[start] < 128) continue;
            var (top, bottom, touchesTop, count) = (height, 0, false, 0);
            labels[start] = regions.Count + 1;
            stack.Push(start);
            while (stack.Count > 0)
            {
                var at = stack.Pop();
                var (x, y) = (at % width, at / width);
                top = Math.Min(top, y);
                bottom = Math.Max(bottom, y);
                touchesTop |= y == 0;
                count++;
                foreach (var next in new[] { x > 0 ? at - 1 : -1, x < width - 1 ? at + 1 : -1, y > 0 ? at - width : -1, y < height - 1 ? at + width : -1 })
                {
                    if (next >= 0 && labels[next] == 0 && mask[next] >= 128)
                    {
                        labels[next] = regions.Count + 1;
                        stack.Push(next);
                    }
                }
            }
            regions.Add((top, bottom, touchesTop, count));
        }
        var main = regions.Where(region => region.TouchesTop).OrderByDescending(region => region.Size).Cast<(int Top, int Bottom, bool TouchesTop, int Size)?>().FirstOrDefault();
        if (main is not { } sky) return;
        for (var i = 0; i < mask.Length; i++)
        {
            if (labels[i] != 0 && regions[labels[i] - 1].Top > sky.Bottom) mask[i] = 0;
        }
    }

    /// <summary>Fills the hole in a 512 x 512 crop. Returns interleaved sRGB bytes.</summary>
    public byte[] Fill(float[] image, float[] mask)
    {
        const int size = Inpaint.Size;
        var planes = Run(Inpaint.File, ("image", image, [1, 3, size, size]), ("mask", mask, [1, 1, size, size]));
        var pixels = size * size;
        var rgb = new byte[pixels * 3];
        for (var i = 0; i < pixels; i++)
        {
            for (var channel = 0; channel < 3; channel++) rgb[i * 3 + channel] = (byte)Math.Clamp(Math.Round(planes[channel * pixels + i]), 0, 255);
        }
        return rgb;
    }

    public void Dispose()
    {
        lock (_lock)
        {
            foreach (var session in _sessions.Values) session.Dispose();
            _sessions.Clear();
        }
    }
}
