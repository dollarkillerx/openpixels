using SkiaSharp;
using Xunit;

namespace OpenPixels.Desktop.Tests;

/// <summary>Uses the git-ignored portrait fixtures and the face models when both are present.</summary>
public sealed class FaceTests(ITestOutputHelper output)
{
    private static string Root => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../.."));

    internal static (byte[] Rgb, int Width, int Height)? LoadRgb(string relative, int longEdge = 1024)
    {
        var path = Path.Combine(Root, relative);
        if (!File.Exists(path)) return null;
        using var original = SKBitmap.Decode(path);
        var scale = Math.Min(1, longEdge / (double)Math.Max(original.Width, original.Height));
        using var bitmap = original.Resize(new SKImageInfo((int)(original.Width * scale), (int)(original.Height * scale), SKColorType.Rgba8888), SKSamplingOptions.Default);
        var rgba = bitmap.Bytes;
        var rgb = new byte[bitmap.Width * bitmap.Height * 3];
        for (var i = 0; i < bitmap.Width * bitmap.Height; i++) (rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]) = (rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]);
        return (rgb, bitmap.Width, bitmap.Height);
    }

    [Theory]
    [InlineData("web/public/fixtures/people/05-woman.jpg", 1)]
    [InlineData("web/public/fixtures/people/09-man.jpg", 1)]
    [InlineData("web/public/fixtures/people/01-acne.jpg", 1)]
    [InlineData("web/public/fixtures/people/03-freckles.jpg", 0)]
    public void Faces_are_found_with_their_landmarks(string file, int count)
    {
        if (!FaceFinder.Available || LoadRgb(file) is not var (rgb, width, height)) return;
        using var finder = new FaceFinder();
        var faces = finder.Find(rgb, width, height, () => LoadRgb(file, 3000)!.Value);
        foreach (var face in faces)
        {
            output.WriteLine($"{file}: box {face.Box} eyes {FaceRegions.EyeRatio(face, width, height, true):0.000}/{FaceRegions.EyeRatio(face, width, height, false):0.000}");
        }
        Assert.Equal(count, faces.Count);
        Assert.All(faces, face => Assert.Equal(478, face.Landmarks.Length));
        Assert.All(faces, face => Assert.False(FaceRegions.EyesClosed(face, width, height)));
    }

    [Fact]
    public void The_skin_mask_covers_the_cheeks_but_not_the_eyes()
    {
        if (!FaceFinder.Available || LoadRgb("web/public/fixtures/people/05-woman.jpg") is not var (rgb, width, height)) return;
        using var finder = new FaceFinder();
        var face = finder.Find(rgb, width, height).First();
        var (mask, faceWidth) = FaceRegions.SkinMask([face], width, height);
        byte At(int index) => mask[(int)(face.Landmarks[index].Y * height) * width + (int)(face.Landmarks[index].X * width)];
        // 205 and 425 are on the cheeks, 468 and 473 the irises, 13 the lips.
        Assert.Equal((255, 255, 0, 0, 0, 0), (At(205), At(425), At(468), At(473), At(13), mask[0]));
        Assert.InRange(faceWidth, width * 0.05, width);
    }

    [Fact]
    public void Culling_counts_faces_and_judges_focus_on_them()
    {
        if (!FaceFinder.Available || LoadRgb("web/public/fixtures/people/05-woman.jpg") is not var (rgb, width, height)) return;
        using var finder = new FaceFinder();
        var faces = finder.Find(rgb, width, height);
        var withFaces = Analysis.Analyze(rgb, width, height, 0, false, faces);
        var without = Analysis.Analyze(rgb, width, height, 0, false);
        Assert.Equal((1, false), (withFaces.Faces, withFaces.EyesClosed));
        Assert.NotEqual(without.Sharpness, withFaces.Sharpness);
    }

    [Fact]
    public void Without_a_face_the_whole_frame_is_skin()
    {
        var (mask, faceWidth) = FaceRegions.SkinMask([], 40, 20);
        Assert.All(mask, value => Assert.Equal(255, value));
        Assert.Equal(14, faceWidth, 3);
    }
}
