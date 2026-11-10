using Microsoft.ML.OnnxRuntime;
using Microsoft.ML.OnnxRuntime.Tensors;

namespace OpenPixels.Desktop;

/// <summary>A face's 478 mesh landmarks, as fractions of the image's width and height.</summary>
public sealed record FaceMesh(UvPoint[] Landmarks)
{
    public (double Left, double Top, double Right, double Bottom) Box =>
        (Landmarks.Min(p => p.X), Landmarks.Min(p => p.Y), Landmarks.Max(p => p.X), Landmarks.Max(p => p.Y));
}

/// <summary>
/// Finds faces and their landmarks with MediaPipe's BlazeFace short-range detector and
/// Face Mesh V2, converted to ONNX. The steps follow MediaPipe's Face Landmarker; the web
/// edition runs the original through @mediapipe/tasks-vision.
/// </summary>
public sealed class FaceFinder : IDisposable
{
    public const string DetectorFile = "face_detector.onnx";
    public const string LandmarksFile = "face_landmarks.onnx";

    private const int DetectorSize = 128;
    private const int MeshSize = 256;
    private const double MinScore = 0.5;
    /// <summary>Overlapping windows mean more chances of a false match, so they demand more.</summary>
    private const double TileScore = 0.8;
    private const double NmsOverlap = 0.3;
    /// <summary>The landmark model sees a square 1.5 times the detected face, turned upright.</summary>
    private const double RoiScale = 1.5;
    private const int MaxFaces = 6;
    private const int TileSize = 512;
    private const int TileStride = 256;
    /// <summary>Canvas sizes for retrying an extreme close-up, relative to its long edge.</summary>
    private static readonly double[] CloseUpPaddings = [2, 3.5];

    private static readonly (double X, double Y)[] Anchors = MakeAnchors();
    private readonly Lazy<InferenceSession> _detector = new(() => new InferenceSession(Path.Combine(AiModels.Folder, DetectorFile)));
    private readonly Lazy<InferenceSession> _mesh = new(() => new InferenceSession(Path.Combine(AiModels.Folder, LandmarksFile)));
    private readonly Lock _lock = new();

    public static bool Available => AiModels.Has(DetectorFile) && AiModels.Has(LandmarksFile);

    /// <summary>
    /// Anchors as MediaPipe's SSD options for the short-range model make them: strides
    /// 8, 16, 16, 16 on a 128 input, two anchors per layer and cell, all of unit size.
    /// </summary>
    private static (double X, double Y)[] MakeAnchors()
    {
        var anchors = new List<(double, double)>();
        foreach (var (stride, layers) in new[] { (8, 1), (16, 3) })
        {
            var cells = DetectorSize / stride;
            for (var y = 0; y < cells; y++)
            {
                for (var x = 0; x < cells; x++)
                {
                    for (var i = 0; i < layers * 2; i++) anchors.Add(((x + 0.5) / cells, (y + 0.5) / cells));
                }
            }
        }
        return [.. anchors];
    }

    private sealed record Detection(double Score, double CentreX, double CentreY, double Width, double Height, (double X, double Y)[] Keypoints);

    /// <summary>Bilinear sample of channel c at (x, y) in image pixels; outside is black.</summary>
    private static double Sample(byte[] rgb, int width, int height, double x, double y, int c)
    {
        x -= 0.5;
        y -= 0.5;
        var (x0, y0) = ((int)Math.Floor(x), (int)Math.Floor(y));
        var (tx, ty) = (x - x0, y - y0);
        double At(int px, int py) => px < 0 || py < 0 || px >= width || py >= height ? 0 : rgb[(py * width + px) * 3 + c];
        return (At(x0, y0) * (1 - tx) + At(x0 + 1, y0) * tx) * (1 - ty) + (At(x0, y0 + 1) * (1 - tx) + At(x0 + 1, y0 + 1) * tx) * ty;
    }

    private static double Sigmoid(double value) => 1 / (1 + Math.Exp(-Math.Clamp(value, -100, 100)));

    /// <summary>Faces in an image, letterboxed into the detector's square; positions as fractions of the image.</summary>
    private List<Detection> Detect(byte[] rgb, int width, int height, double minScore)
    {
        var scale = DetectorSize / (double)Math.Max(width, height);
        var (left, top) = ((DetectorSize - width * scale) / 2, (DetectorSize - height * scale) / 2);
        var input = new float[DetectorSize * DetectorSize * 3];
        for (var y = 0; y < DetectorSize; y++)
        {
            for (var x = 0; x < DetectorSize; x++)
            {
                var (sx, sy) = ((x + 0.5 - left) / scale, (y + 0.5 - top) / scale);
                for (var c = 0; c < 3; c++) input[(y * DetectorSize + x) * 3 + c] = (float)(Sample(rgb, width, height, sx, sy, c) / 127.5 - 1);
            }
        }
        float[] boxes, scores;
        lock (_lock)
        {
            using var results = _detector.Value.Run([NamedOnnxValue.CreateFromTensor("input", new DenseTensor<float>(input, [1, DetectorSize, DetectorSize, 3]))]);
            boxes = results.First(result => result.Name == "regressors").AsTensor<float>().ToArray();
            scores = results.First(result => result.Name == "classificators").AsTensor<float>().ToArray();
        }
        var found = new List<Detection>();
        for (var i = 0; i < Anchors.Length; i++)
        {
            var score = Sigmoid(scores[i]);
            if (score < minScore) continue;
            var raw = boxes[(i * 16)..(i * 16 + 16)];
            var (ax, ay) = Anchors[i];
            // Detector space (0..1 of the letterboxed square) back to the image.
            (double, double) ToImage(double dx, double dy) => (((dx * DetectorSize) - left) / scale / width, ((dy * DetectorSize) - top) / scale / height);
            var (cx, cy) = ToImage(raw[0] / DetectorSize + ax, raw[1] / DetectorSize + ay);
            var keypoints = Enumerable.Range(0, 6).Select(k => ToImage(raw[4 + k * 2] / DetectorSize + ax, raw[5 + k * 2] / DetectorSize + ay)).ToArray();
            found.Add(new Detection(score, cx, cy, raw[2] / scale / width, raw[3] / scale / height, keypoints));
        }
        return WeightedNms(found);
    }

    private static double Overlap(Detection a, Detection b)
    {
        var w = Math.Min(a.CentreX + a.Width / 2, b.CentreX + b.Width / 2) - Math.Max(a.CentreX - a.Width / 2, b.CentreX - b.Width / 2);
        var h = Math.Min(a.CentreY + a.Height / 2, b.CentreY + b.Height / 2) - Math.Max(a.CentreY - a.Height / 2, b.CentreY - b.Height / 2);
        if (w <= 0 || h <= 0) return 0;
        var shared = w * h;
        return shared / (a.Width * a.Height + b.Width * b.Height - shared);
    }

    /// <summary>MediaPipe's weighted non-maximum suppression: overlapping boxes are averaged by score.</summary>
    private static List<Detection> WeightedNms(List<Detection> detections)
    {
        var remaining = detections.OrderByDescending(d => d.Score).ToList();
        var kept = new List<Detection>();
        while (remaining.Count > 0 && kept.Count < MaxFaces)
        {
            var best = remaining[0];
            var cluster = remaining.Where(d => Overlap(best, d) > NmsOverlap).ToList();
            remaining = remaining.Except(cluster).ToList();
            var total = cluster.Sum(d => d.Score);
            double Mean(Func<Detection, double> value) => cluster.Sum(d => value(d) * d.Score) / total;
            kept.Add(new Detection(best.Score, Mean(d => d.CentreX), Mean(d => d.CentreY), Mean(d => d.Width), Mean(d => d.Height),
                Enumerable.Range(0, 6).Select(k => (Mean(d => d.Keypoints[k].X), Mean(d => d.Keypoints[k].Y))).ToArray()));
        }
        return kept;
    }

    /// <summary>The face's landmarks, from an upright square around it, or null if the mesh model sees no face.</summary>
    private FaceMesh? Landmarks(byte[] rgb, int width, int height, Detection face)
    {
        // In pixels: the square is set by the long side, turned so the eyes are level.
        var (rightEye, leftEye) = (face.Keypoints[0], face.Keypoints[1]);
        var rotation = -Math.Atan2(-(leftEye.Y - rightEye.Y) * height, (leftEye.X - rightEye.X) * width);
        var size = Math.Max(face.Width * width, face.Height * height) * RoiScale;
        var (cx, cy) = (face.CentreX * width, face.CentreY * height);
        var (cos, sin) = (Math.Cos(rotation), Math.Sin(rotation));
        (double X, double Y) FromCrop(double u, double v)
        {
            var (dx, dy) = ((u / MeshSize - 0.5) * size, (v / MeshSize - 0.5) * size);
            return (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
        }
        var input = new float[MeshSize * MeshSize * 3];
        for (var v = 0; v < MeshSize; v++)
        {
            for (var u = 0; u < MeshSize; u++)
            {
                var (sx, sy) = FromCrop(u + 0.5, v + 0.5);
                for (var c = 0; c < 3; c++) input[(v * MeshSize + u) * 3 + c] = (float)(Sample(rgb, width, height, sx, sy, c) / 255);
            }
        }
        float[] points;
        float presence;
        lock (_lock)
        {
            var session = _mesh.Value;
            using var results = session.Run([NamedOnnxValue.CreateFromTensor(session.InputNames[0], new DenseTensor<float>(input, [1, MeshSize, MeshSize, 3]))]);
            points = results.First(result => result.Name == "Identity").AsTensor<float>().ToArray();
            presence = results.First(result => result.Name == "Identity_1").AsTensor<float>().ToArray()[0];
        }
        if (Sigmoid(presence) < MinScore) return null;
        var landmarks = new UvPoint[points.Length / 3];
        for (var i = 0; i < landmarks.Length; i++)
        {
            var (x, y) = FromCrop(points[i * 3], points[i * 3 + 1]);
            landmarks[i] = new UvPoint(x / width, y / height);
        }
        return new FaceMesh(landmarks);
    }

    private List<FaceMesh> Meshes(byte[] rgb, int width, int height, double minScore) =>
        Detect(rgb, width, height, minScore).Select(face => Landmarks(rgb, width, height, face)).OfType<FaceMesh>().ToList();

    /// <summary>
    /// Faces of any size: as they are, then extreme close-ups given room around them, then
    /// faces too small to see whole, searched in overlapping windows of a larger image.
    /// </summary>
    public List<FaceMesh> Find(byte[] rgb, int width, int height, Func<(byte[] Rgb, int Width, int Height)>? detailed = null)
    {
        var whole = Meshes(rgb, width, height, MinScore);
        if (whole.Count > 0) return whole;
        foreach (var padding in CloseUpPaddings)
        {
            var size = (int)Math.Round(Math.Max(width, height) * padding);
            var (left, top) = ((size - width) / 2, (size - height) / 2);
            var canvas = new byte[size * size * 3];
            Array.Fill(canvas, (byte)128);
            for (var y = 0; y < height; y++) Buffer.BlockCopy(rgb, y * width * 3, canvas, ((y + top) * size + left) * 3, width * 3);
            var padded = Meshes(canvas, size, size, MinScore);
            if (padded.Count > 0)
            {
                return padded.Select(face => new FaceMesh(face.Landmarks.Select(p => new UvPoint((p.X * size - left) / width, (p.Y * size - top) / height)).ToArray())).ToList();
            }
        }
        var (bigRgb, bigWidth, bigHeight) = detailed?.Invoke() ?? (rgb, width, height);
        return FindInTiles(bigRgb, bigWidth, bigHeight);
    }

    private List<FaceMesh> FindInTiles(byte[] rgb, int width, int height)
    {
        static List<int> Steps(int size)
        {
            var last = Math.Max(0, size - TileSize);
            var positions = new List<int>();
            for (var at = 0; at < last; at += TileStride) positions.Add(at);
            positions.Add(last);
            return positions;
        }
        var found = new List<FaceMesh>();
        var (tileWidth, tileHeight) = (Math.Min(TileSize, width), Math.Min(TileSize, height));
        var tile = new byte[tileWidth * tileHeight * 3];
        foreach (var top in Steps(height))
        {
            foreach (var left in Steps(width))
            {
                for (var y = 0; y < tileHeight; y++) Buffer.BlockCopy(rgb, ((top + y) * width + left) * 3, tile, y * tileWidth * 3, tileWidth * 3);
                foreach (var face in Meshes(tile, tileWidth, tileHeight, TileScore))
                {
                    var mapped = new FaceMesh(face.Landmarks.Select(p => new UvPoint((left + p.X * tileWidth) / width, (top + p.Y * tileHeight) / height)).ToArray());
                    var centre = (X: mapped.Landmarks.Average(p => p.X) * width, Y: mapped.Landmarks.Average(p => p.Y) * height);
                    // Overlapping windows see the same face more than once.
                    var duplicate = found.Any(other =>
                        Math.Sqrt(Math.Pow(other.Landmarks.Average(p => p.X) * width - centre.X, 2) + Math.Pow(other.Landmarks.Average(p => p.Y) * height - centre.Y, 2)) < TileStride / 2.0);
                    if (!duplicate) found.Add(mapped);
                }
            }
        }
        return found;
    }

    public void Dispose()
    {
        if (_detector.IsValueCreated) _detector.Value.Dispose();
        if (_mesh.IsValueCreated) _mesh.Value.Dispose();
    }
}

/// <summary>What the face landmarks tell the editor: skin to retouch, and whether eyes are shut.</summary>
public static class FaceRegions
{
    // MediaPipe's Face Mesh index sets, as in its face_mesh_connections.
    private static readonly int[] FaceOval =
        [10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109];
    private static readonly int[] LeftEye = [263, 249, 390, 373, 374, 380, 381, 382, 362, 466, 388, 387, 386, 385, 384, 398];
    private static readonly int[] RightEye = [33, 7, 163, 144, 145, 153, 154, 155, 133, 246, 161, 160, 159, 158, 157, 173];
    private static readonly int[] LeftBrow = [276, 283, 282, 295, 285, 300, 293, 334, 296, 336];
    private static readonly int[] RightBrow = [46, 53, 52, 65, 55, 70, 63, 105, 66, 107];
    private static readonly int[] Lips =
        [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 185, 40, 39, 37, 0, 267, 269, 270, 409, 415, 310, 311, 312, 13, 82, 81, 42, 183, 78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308];
    /// <summary>Facial features kept out of the skin mask, and how much to grow each, as in the web edition.</summary>
    private static readonly (int[] Indices, double Grow)[] Features = [(LeftEye, 1.4), (RightEye, 1.4), (LeftBrow, 1.25), (RightBrow, 1.25), (Lips, 1.15)];
    /// <summary>Nostrils are dark spots that are not blemishes.</summary>
    private static readonly int[] Nostrils = [79, 309];
    private const double NostrilRadius = 0.05;
    /// <summary>Assumed face size when none is found, as a fraction of the short edge.</summary>
    private const double CloseUpFaceFraction = 0.7;

    // Eye corners and lid pairs for the eye aspect ratio.
    private static readonly int[] RightEyeRatio = [33, 160, 158, 133, 153, 144];
    private static readonly int[] LeftEyeRatio = [263, 387, 385, 362, 380, 373];
    /// <summary>
    /// Below this eye aspect ratio an eye counts as shut. Smiling eyes narrow to about 0.17,
    /// shut ones are below 0.1.
    /// </summary>
    public const double ClosedEyeRatio = 0.13;

    private static List<(double X, double Y)> Hull(IEnumerable<(double X, double Y)> points)
    {
        var sorted = points.OrderBy(p => p.X).ThenBy(p => p.Y).ToList();
        static double Cross((double X, double Y) o, (double X, double Y) a, (double X, double Y) b) => (a.X - o.X) * (b.Y - o.Y) - (a.Y - o.Y) * (b.X - o.X);
        List<(double, double)> Half(IEnumerable<(double X, double Y)> input)
        {
            var chain = new List<(double X, double Y)>();
            foreach (var point in input)
            {
                while (chain.Count >= 2 && Cross(chain[^2], chain[^1], point) <= 0) chain.RemoveAt(chain.Count - 1);
                chain.Add(point);
            }
            chain.RemoveAt(chain.Count - 1);
            return chain;
        }
        return [.. Half(sorted), .. Half(Enumerable.Reverse(sorted))];
    }

    private static List<(double X, double Y)> Grown(List<(double X, double Y)> polygon, double factor)
    {
        var (cx, cy) = (polygon.Average(p => p.X), polygon.Average(p => p.Y));
        return polygon.Select(p => (cx + (p.X - cx) * factor, cy + (p.Y - cy) * factor)).ToList();
    }

    /// <summary>Fills a convex polygon, in pixels, with a value.</summary>
    private static void Fill(byte[] mask, int width, int height, List<(double X, double Y)> polygon, byte value)
    {
        if (polygon.Count < 3) return;
        var (top, bottom) = (Math.Max(0, (int)Math.Floor(polygon.Min(p => p.Y))), Math.Min(height - 1, (int)Math.Ceiling(polygon.Max(p => p.Y))));
        for (var y = top; y <= bottom; y++)
        {
            var scan = y + 0.5;
            double left = double.MaxValue, right = double.MinValue;
            for (var i = 0; i < polygon.Count; i++)
            {
                var (a, b) = (polygon[i], polygon[(i + 1) % polygon.Count]);
                if ((a.Y <= scan && b.Y > scan) || (b.Y <= scan && a.Y > scan))
                {
                    var x = a.X + (scan - a.Y) / (b.Y - a.Y) * (b.X - a.X);
                    (left, right) = (Math.Min(left, x), Math.Max(right, x));
                }
            }
            if (left > right) continue;
            for (var x = Math.Max(0, (int)Math.Ceiling(left - 0.5)); x <= Math.Min(width - 1, (int)Math.Floor(right - 0.5)); x++) mask[y * width + x] = value;
        }
    }

    /// <summary>
    /// The skin of every face at the given size, and the typical face width in pixels. With
    /// no face, as in a profile or a tight close-up, the whole frame is passed on and the
    /// retouch's own colour checks decide what is skin.
    /// </summary>
    public static (byte[] Mask, double FaceWidth) SkinMask(IReadOnlyList<FaceMesh> faces, int width, int height)
    {
        var mask = new byte[width * height];
        if (faces.Count == 0)
        {
            Array.Fill(mask, (byte)255);
            return (mask, Math.Min(width, height) * CloseUpFaceFraction);
        }
        var widths = new List<double>();
        foreach (var face in faces)
        {
            (double X, double Y) Point(int index) => (face.Landmarks[index].X * width, face.Landmarks[index].Y * height);
            var oval = Hull(FaceOval.Select(Point));
            Fill(mask, width, height, oval, 255);
            foreach (var (indices, grow) in Features) Fill(mask, width, height, Grown(Hull(indices.Select(Point)), grow), 0);
            var faceWidth = oval.Max(p => p.X) - oval.Min(p => p.X);
            foreach (var index in Nostrils)
            {
                var (x, y) = Point(index);
                var radius = faceWidth * NostrilRadius;
                Fill(mask, width, height, Enumerable.Range(0, 16).Select(i => (x + radius * Math.Cos(i * Math.PI / 8), y + radius * Math.Sin(i * Math.PI / 8))).ToList(), 0);
            }
            widths.Add(faceWidth);
        }
        widths.Sort();
        return (mask, widths[widths.Count / 2]);
    }

    /// <summary>
    /// How open an eye is: the lids' gap over the eye's width. Open eyes are about 0.25 to
    /// 0.35. The web edition reads MediaPipe's blink blendshapes instead.
    /// </summary>
    public static double EyeRatio(FaceMesh face, int width, int height, bool left)
    {
        var indices = left ? LeftEyeRatio : RightEyeRatio;
        (double X, double Y) P(int i) => (face.Landmarks[indices[i]].X * width, face.Landmarks[indices[i]].Y * height);
        static double Distance((double X, double Y) a, (double X, double Y) b) => Math.Sqrt(Math.Pow(a.X - b.X, 2) + Math.Pow(a.Y - b.Y, 2));
        return (Distance(P(1), P(5)) + Distance(P(2), P(4))) / (2 * Math.Max(1e-6, Distance(P(0), P(3))));
    }

    /// <summary>Both eyes shut, as a blink in a portrait would leave them.</summary>
    public static bool EyesClosed(FaceMesh face, int width, int height) =>
        Math.Max(EyeRatio(face, width, height, left: true), EyeRatio(face, width, height, left: false)) < ClosedEyeRatio;
}
