using System.Runtime.InteropServices;

namespace OpenPixels.Desktop.Engine;

/// <summary>
/// A decoded image as RGBA, 8 or 16 bits per channel (16-bit samples native-endian), in
/// the colour space an ICC profile describes, or "p3", or sRGB when neither is given.
/// </summary>
public sealed record DecodedRaster(int Width, int Height, byte[] Rgba, int Depth = 8, string Space = "srgb", byte[]? Icc = null);

/// <summary>
/// HEIC, which the engine cannot read itself. HEVC needs a patent licence, so macOS uses
/// its own decoder. Elsewhere libheif with the libde265 decoder (both LGPL) is loaded at
/// run time: the Windows package carries it, Linux uses the system's (libheif1).
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

    public static bool CanDecodeHeic => OperatingSystem.IsMacOS() || LibHeif.Available;

    /// <summary>Decodes with the system's decoder, rotation applied. Throws UserMessageException if it cannot.</summary>
    public static DecodedRaster Decode(byte[] bytes)
    {
        if (OperatingSystem.IsMacOS() && Environment.GetEnvironmentVariable("OPENPIXELS_HEIC") != "libheif") return ImageIO.Decode(bytes);
        if (LibHeif.Available) return LibHeif.Decode(bytes);
        throw new UserMessageException(OperatingSystem.IsLinux()
            ? I18n.T("打开 HEIC 需要 libheif，请先安装（例如 sudo apt install libheif1）")
            : I18n.T("这个系统暂时不能打开 HEIC，请先转换为 JPEG"));
    }

    /// <summary>macOS ImageIO and Core Graphics.</summary>
    private static unsafe class ImageIO
    {
        private const string CoreFoundationPath = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";
        private const string ImageIOPath = "/System/Library/Frameworks/ImageIO.framework/ImageIO";
        private const string CoreGraphicsPath = "/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics";
        /// <summary>kCGImageAlphaPremultipliedLast: RGBA in memory order.</summary>
        private const uint PremultipliedLast = 1;
        /// <summary>kCGBitmapByteOrder16Little, for 16-bit samples in this machine's order.</summary>
        private const uint ByteOrder16Little = 1 << 12;

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
        [DllImport(CoreGraphicsPath)] private static extern nint CGImageGetColorSpace(nint image);
        [DllImport(CoreGraphicsPath)] [return: MarshalAs(UnmanagedType.U1)] private static extern bool CGColorSpaceIsWideGamutRGB(nint space);
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
                if (source == 0) throw new UserMessageException(I18n.T("无法读取这个 HEIC 文件"));
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
                if (image == 0) throw new UserMessageException(I18n.T("HEIC 文件里没有可解码的图像"));
                var (width, height) = ((int)CGImageGetWidth(image), (int)CGImageGetHeight(image));
                // Drawing converts from the file's colour space. Wide ones, usually Display P3,
                // are kept as P3 in 16 bits, so colours beyond sRGB survive; others become sRGB.
                var fileSpace = CGImageGetColorSpace(image);
                var wide = fileSpace != 0 && CGColorSpaceIsWideGamutRGB(fileSpace);
                var depth = wide ? 16 : 8;
                var rgba = new byte[width * height * 4 * (depth / 8)];
                space = CGColorSpaceCreateWithName(Constant(CoreGraphicsPath, wide ? "kCGColorSpaceDisplayP3" : "kCGColorSpaceSRGB"));
                fixed (byte* pixels = rgba)
                {
                    var bytesPerRow = (nuint)(width * 4 * (depth / 8));
                    context = CGBitmapContextCreate(pixels, (nuint)width, (nuint)height, (nuint)depth, bytesPerRow, space, PremultipliedLast | (wide ? ByteOrder16Little : 0));
                    if (context == 0) throw new UserMessageException(I18n.T("HEIC 解码失败"));
                    CGContextDrawImage(context, new CGRect(0, 0, width, height), image);
                }
                return new DecodedRaster(width, height, rgba, depth, wide ? "p3" : "srgb");
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

    /// <summary>libheif's C API, found at run time; see https://github.com/strukturag/libheif.</summary>
    private static unsafe class LibHeif
    {
        private const int ColorspaceRgb = 1;
        private const int ChromaInterleavedRgba = 11;
        private const int ChromaInterleavedRrggbbaaLe = 15;
        private const int ChannelInterleaved = 10;
        private static readonly int ProfileNclx = FourCc("nclx");
        private static readonly int ProfileRawIcc = FourCc("rICC");
        private static readonly int ProfileIcc = FourCc("prof");
        /// <summary>ITU-T H.273 colour primaries: SMPTE EG 432-1, which is Display P3.</summary>
        private const int PrimariesDisplayP3 = 12;

        [StructLayout(LayoutKind.Sequential)]
        private struct HeifError
        {
            public int Code;
            public int Subcode;
            public nint Message;
        }

        private static int FourCc(string code) => (code[0] << 24) | (code[1] << 16) | (code[2] << 8) | code[3];

        private static readonly nint Library = Load();

        private static nint Load()
        {
            string[] names = OperatingSystem.IsWindows() ? ["heif.dll", "libheif.dll"]
                : OperatingSystem.IsMacOS() ? ["libheif.1.dylib", "/opt/homebrew/lib/libheif.1.dylib", "/usr/local/lib/libheif.1.dylib"]
                : ["libheif.so.1", "libheif.so"];
            foreach (var name in names)
            {
                // The app's own folder first, then the system's search path.
                var local = Path.Combine(AppContext.BaseDirectory, name);
                if (NativeLibrary.TryLoad(File.Exists(local) ? local : name, out var handle)) return handle;
            }
            return 0;
        }

        public static bool Available => Library != 0;

        private static nint Export(string name) => NativeLibrary.GetExport(Library, name);

        private static nint TryExport(string name) => NativeLibrary.TryGetExport(Library, name, out var address) ? address : 0;

        private static int _initialised;

        private static void Check(HeifError error)
        {
            if (error.Code == 0) return;
            var message = Marshal.PtrToStringUTF8(error.Message) ?? $"libheif error {error.Code}";
            throw new UserMessageException(I18n.T("HEIC 解码失败") + $" ({message})");
        }

        public static DecodedRaster Decode(byte[] bytes)
        {
            // heif_init loads decoder plugins in libheif 1.14 and later; older versions have none.
            if (Interlocked.Exchange(ref _initialised, 1) == 0 && TryExport("heif_init") is var init and not 0)
            {
                ((delegate* unmanaged<nint, HeifError>)init)(0);
            }
            var contextAlloc = (delegate* unmanaged<nint>)Export("heif_context_alloc");
            var contextFree = (delegate* unmanaged<nint, void>)Export("heif_context_free");
            var readFromMemory = (delegate* unmanaged<nint, byte*, nuint, nint, HeifError>)Export("heif_context_read_from_memory_without_copy");
            var primaryHandle = (delegate* unmanaged<nint, nint*, HeifError>)Export("heif_context_get_primary_image_handle");
            var handleRelease = (delegate* unmanaged<nint, void>)Export("heif_image_handle_release");
            var lumaBits = (delegate* unmanaged<nint, int>)Export("heif_image_handle_get_luma_bits_per_pixel");
            var profileType = (delegate* unmanaged<nint, int>)Export("heif_image_handle_get_color_profile_type");
            var rawProfileSize = (delegate* unmanaged<nint, nuint>)Export("heif_image_handle_get_raw_color_profile_size");
            var rawProfile = (delegate* unmanaged<nint, byte*, HeifError>)Export("heif_image_handle_get_raw_color_profile");
            var nclxProfile = (delegate* unmanaged<nint, nint*, HeifError>)Export("heif_image_handle_get_nclx_color_profile");
            var nclxFree = (delegate* unmanaged<nint, void>)Export("heif_nclx_color_profile_free");
            var decodeImage = (delegate* unmanaged<nint, nint*, int, int, nint, HeifError>)Export("heif_decode_image");
            var imageRelease = (delegate* unmanaged<nint, void>)Export("heif_image_release");
            var plane = (delegate* unmanaged<nint, int, int*, byte*>)Export("heif_image_get_plane_readonly");
            var imageWidth = (delegate* unmanaged<nint, int, int>)Export("heif_image_get_width");
            var imageHeight = (delegate* unmanaged<nint, int, int>)Export("heif_image_get_height");
            var bitsRange = (delegate* unmanaged<nint, int, int>)Export("heif_image_get_bits_per_pixel_range");

            nint context = contextAlloc(), handle = 0, image = 0;
            try
            {
                fixed (byte* data = bytes)
                {
                    Check(readFromMemory(context, data, (nuint)bytes.Length, 0));
                    Check(primaryHandle(context, &handle));

                    byte[]? icc = null;
                    var space = "srgb";
                    var type = profileType(handle);
                    if (type == ProfileRawIcc || type == ProfileIcc)
                    {
                        icc = new byte[(int)rawProfileSize(handle)];
                        fixed (byte* profile = icc) Check(rawProfile(handle, profile));
                    }
                    else if (type == ProfileNclx)
                    {
                        nint nclx = 0;
                        if (nclxProfile(handle, &nclx).Code == 0 && nclx != 0)
                        {
                            // { uint8 version; int color_primaries; ... }
                            if (*(int*)(nclx + 4) == PrimariesDisplayP3) space = "p3";
                            nclxFree(nclx);
                        }
                    }

                    // Deeper images, such as 10-bit HEIC, keep their precision.
                    var deep = lumaBits(handle) > 8;
                    // Decoding applies the file's rotation and mirroring.
                    Check(decodeImage(handle, &image, ColorspaceRgb, deep ? ChromaInterleavedRrggbbaaLe : ChromaInterleavedRgba, 0));
                    var (width, height) = (imageWidth(image, ChannelInterleaved), imageHeight(image, ChannelInterleaved));
                    int stride;
                    var pixels = plane(image, ChannelInterleaved, &stride);
                    if (pixels == null || width <= 0 || height <= 0) throw new UserMessageException(I18n.T("HEIC 文件里没有可解码的图像"));
                    var rowBytes = width * 4 * (deep ? 2 : 1);
                    var rgba = new byte[rowBytes * height];
                    for (var y = 0; y < height; y++) new ReadOnlySpan<byte>(pixels + (long)y * stride, rowBytes).CopyTo(rgba.AsSpan(y * rowBytes));
                    if (deep)
                    {
                        // Samples are in the image's own range, such as 0..1023; spread them over 16 bits.
                        var bits = Math.Clamp(bitsRange(image, ChannelInterleaved), 9, 16);
                        var samples = MemoryMarshal.Cast<byte, ushort>(rgba.AsSpan());
                        for (var i = 0; i < samples.Length; i++)
                        {
                            var value = BitConverter.IsLittleEndian ? samples[i] : System.Buffers.Binary.BinaryPrimitives.ReverseEndianness(samples[i]);
                            samples[i] = (ushort)((value << (16 - bits)) | (value >> (2 * bits - 16)));
                        }
                    }
                    return new DecodedRaster(width, height, rgba, deep ? 16 : 8, space, icc);
                }
            }
            finally
            {
                if (image != 0) imageRelease(image);
                if (handle != 0) handleRelease(handle);
                contextFree(context);
            }
        }
    }
}
