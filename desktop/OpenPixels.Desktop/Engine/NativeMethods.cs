using System.Runtime.InteropServices;

namespace OpenPixels.Desktop.Engine;

/// <summary>A byte buffer owned by the engine; give it back with <see cref="NativeMethods.Free"/>.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct OpBuffer
{
    public nint Ptr;
    public nuint Len;

    public readonly byte[] ToArray()
    {
        if (Ptr == 0 || Len == 0) return [];
        var bytes = new byte[(int)Len];
        Marshal.Copy(Ptr, bytes, 0, bytes.Length);
        return bytes;
    }
}

/// <summary>The C entry points of the Rust crate op-native.</summary>
internal static unsafe partial class NativeMethods
{
    private const string Library = "op_native";

    [LibraryImport(Library, EntryPoint = "op_engine_create")]
    internal static partial nint Create(out OpBuffer error);

    [LibraryImport(Library, EntryPoint = "op_engine_destroy")]
    internal static partial void Destroy(nint engine);

    [LibraryImport(Library, EntryPoint = "op_engine_call")]
    internal static partial int Call(
        nint engine,
        byte* method, nuint methodLength,
        byte* args, nuint argsLength,
        byte* data, nuint dataLength,
        out OpBuffer json, out OpBuffer output);

    [LibraryImport(Library, EntryPoint = "op_buffer_free")]
    internal static partial void Free(OpBuffer buffer);
}
