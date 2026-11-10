using System.Runtime.CompilerServices;

namespace OpenPixels.Desktop.Tests;

internal static class TestLanguage
{
    /// <summary>Tests read the Chinese interface text, whatever language this machine uses.</summary>
    [ModuleInitializer]
    internal static void UseChinese() => Environment.SetEnvironmentVariable("OPENPIXELS_LANGUAGE", "zh-CN");
}
