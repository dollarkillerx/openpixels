using System.Runtime.InteropServices;

namespace OpenPixels.Desktop.Engine;

/// <summary>An image decoded by the operating system, as 8-bit sRGB RGBA.</summary>
public sealed record DecodedRaster(int Width, int Height, byte[] Rgba);

/// <summary>
/// Formats the engine cannot read itself but the system can. HEIC uses HEVC, which needs a
/// patent licence, so the system's own decoder is used rather than shipping one.
/// </summary>
public static class PlatformDecoder
{
    private static readonly HashSet<string> HevcBrands = ["heic", "heix", "hevc", "hevx", "heim", "heis", "hevm", "hevs"];
    private static readonly HashSet<string> ContainerBrands = ["mif1", "msf1"];

    /// <summary>True for HEIF files holding HEVC images, which rules out AVIF sharing the container.</summary>
    public static bool IsHeic(ReadOnlySpan<byte> bytes)
    {
        static string Brand(ReadOnlySpan<byte> data, int at) => System.Text.Encoding.ASCII.GetString(data.Slice(at, 4));
        if (bytes.Length < 16 || Brand(bytes, 4) != "ftyp") return false;
        var major = Brand(bytes, 8);
        if (HevcBrands.Contains(major)) return true;
        if (!ContainerBrands.Contains(major)) return false;
        var boxSize = Math.Min((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3], bytes.Length);
        for (var at = 16; at + 4 <= boxSize; at += 4)
        {
            if (HevcBrands.Contains(Brand(bytes, at))) return true;
        }
        return false;
    }

    public static bool CanDecodeHeic => OperatingSystem.IsMacOS();

    /// <summary>Decodes with the system's decoder, rotation applied. Throws EngineException if it cannot.</summary>
    public static DecodedRaster Decode(byte[] bytes)
    {
        if (OperatingSystem.IsMacOS()) return ImageIO.Decode(bytes);
        throw new EngineException("这个系统暂时不能打开 HEIC，请先转换为 JPEG");
    }

    /// <summary>macOS ImageIO and Core Graphics.</summary>
    private static unsafe class ImageIO
    {
        private const string CoreFoundationPath = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";
        private const string ImageIOPath = "/System/Library/Frameworks/ImageIO.framework/ImageIO";
        private const string CoreGraphicsPath = "/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics";
        /// <summary>kCGImageAlphaPremultipliedLast: RGBA in memory order.</summary>
        private const uint PremultipliedLast = 1;

        [StructLayout(LayoutKind.Sequential)]
        private struct CGRect(double x, double y, double width, double height)
        {
            public double X = x, Y = y, Width = width, Height = height;
        }

        [DllImport(CoreFoundationPath)] private static extern nint CFDataCreate(nint allocator, byte* bytes, nint length);
        [DllImport(CoreFoundationPath)] private static extern void CFRelease(nint value);
        [DllImport(CoreFoundationPath)] private static extern nint CFDictionaryCreate(nint allocator, nint* keys, nint* values, nint count, nint keyCallbacks, nint valueCallbacks);
        [DllImport(ImageIOPath)] private static extern nint CGImageSourceCreateWithData(nint data, nint options);
        [DllImport(ImageIOPath)] private static extern nint CGImageSourceCreateThumbnailAtIndex(nint source, nint index, nint options);
        [DllImport(CoreGraphicsPath)] private static extern nuint CGImageGetWidth(nint image);
        [DllImport(CoreGraphicsPath)] private static extern nuint CGImageGetHeight(nint image);
        [DllImport(CoreGraphicsPath)] private static extern nint CGColorSpaceCreateWithName(nint name);
        [DllImport(CoreGraphicsPath)] private static extern nint CGBitmapContextCreate(byte* data, nuint width, nuint height, nuint bitsPerComponent, nuint bytesPerRow, nint space, uint bitmapInfo);
        [DllImport(CoreGraphicsPath)] private static extern void CGContextDrawImage(nint context, CGRect rect, nint image);
        [DllImport(CoreGraphicsPath)] private static extern void CGContextRelease(nint context);
        [DllImport(CoreGraphicsPath)] private static extern void CGColorSpaceRelease(nint space);
        [DllImport(CoreGraphicsPath)] private static extern void CGImageRelease(nint image);

        /// <summary>The value of an exported constant such as kCFBooleanTrue.</summary>
        private static nint Constant(string library, string name) => *(nint*)NativeLibrary.GetExport(NativeLibrary.Load(library), name);

        private static nint Address(string library, string name) => NativeLibrary.GetExport(NativeLibrary.Load(library), name);

        public static DecodedRaster Decode(byte[] bytes)
        {
            nint data, source = 0, options = 0, image = 0, space = 0, context = 0;
            fixed (byte* pointer = bytes) data = CFDataCreate(0, pointer, bytes.Length);
            try
            {
                source = CGImageSourceCreateWithData(data, 0);
                if (source == 0) throw new EngineException("无法读取这个 HEIC 文件");
                // A "thumbnail" with no size limit is the full image with its rotation applied.
                var yes = Constant(CoreFoundationPath, "kCFBooleanTrue");
                var keys = stackalloc nint[]
                {
                    Constant(ImageIOPath, "kCGImageSourceCreateThumbnailFromImageAlways"),
                    Constant(ImageIOPath, "kCGImageSourceCreateThumbnailWithTransform"),
                };
                var values = stackalloc nint[] { yes, yes };
                options = CFDictionaryCreate(0, keys, values, 2, Address(CoreFoundationPath, "kCFTypeDictionaryKeyCallBacks"), Address(CoreFoundationPath, "kCFTypeDictionaryValueCallBacks"));
                image = CGImageSourceCreateThumbnailAtIndex(source, 0, options);
                if (image == 0) throw new EngineException("HEIC 文件里没有可解码的图像");
                var (width, height) = ((int)CGImageGetWidth(image), (int)CGImageGetHeight(image));
                var rgba = new byte[width * height * 4];
                // Drawing into an sRGB context converts from the file's colour space (often Display P3).
                space = CGColorSpaceCreateWithName(Constant(CoreGraphicsPath, "kCGColorSpaceSRGB"));
                fixed (byte* pixels = rgba)
                {
                    context = CGBitmapContextCreate(pixels, (nuint)width, (nuint)height, 8, (nuint)(width * 4), space, PremultipliedLast);
                    if (context == 0) throw new EngineException("HEIC 解码失败");
                    CGContextDrawImage(context, new CGRect(0, 0, width, height), image);
                }
                for (var at = 3; at < rgba.Length; at += 4) rgba[at] = 255;
                return new DecodedRaster(width, height, rgba);
            }
            finally
            {
                if (context != 0) CGContextRelease(context);
                if (space != 0) CGColorSpaceRelease(space);
                if (image != 0) CGImageRelease(image);
                if (options != 0) CFRelease(options);
                if (source != 0) CFRelease(source);
                if (data != 0) CFRelease(data);
            }
        }
    }
}
