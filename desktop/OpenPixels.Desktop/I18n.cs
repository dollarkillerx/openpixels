using System.Globalization;
using System.Reflection;
using System.Text.Json;
using System.Text.RegularExpressions;
using Avalonia.Markup.Xaml;

namespace OpenPixels.Desktop;

/// <summary>
/// Interface languages, shared with the web edition. The Simplified Chinese text is the
/// key, and web/src/i18n/{language}.json maps it to a translation; those files are built
/// into the app. A text a language lacks falls back to English, then to the Chinese.
/// </summary>
public static partial class I18n
{
    public static readonly (string Code, string Name)[] Languages =
        [("zh-CN", "简体中文"), ("zh-TW", "繁體中文"), ("en", "English"), ("ja", "日本語"), ("ko", "한국어")];

    private const string Fallback = "en";
    /// <summary>Made on use, so a changed OPENPIXELS_DATA_DIR is honoured.</summary>
    private static JsonFile<Dictionary<string, string>> Settings => new("settings.json");
    private static readonly Lock Gate = new();
    private static Dictionary<string, string>? _table;
    private static Dictionary<string, string>? _fallback;
    private static string? _language;

    /// <summary>The language in use; chosen on first use and kept until the app restarts.</summary>
    public static string Language
    {
        get
        {
            Initialize();
            return _language!;
        }
    }

    /// <summary>The best match for a culture name, or null. Any Chinese that is not Traditional is Simplified.</summary>
    public static string? Match(string culture)
    {
        var lower = culture.ToLowerInvariant();
        if (lower.StartsWith("zh", StringComparison.Ordinal)) return Regex.IsMatch(lower, "hant|tw|hk|mo") ? "zh-TW" : "zh-CN";
        if (lower.StartsWith("ja", StringComparison.Ordinal)) return "ja";
        if (lower.StartsWith("ko", StringComparison.Ordinal)) return "ko";
        if (lower.StartsWith("en", StringComparison.Ordinal)) return "en";
        return null;
    }

    private static bool Known(string? code) => Languages.Any(language => language.Code == code);

    /// <summary>OPENPIXELS_LANGUAGE first, for tests; then the saved choice; then the system's language.</summary>
    private static string Choose()
    {
        if (Environment.GetEnvironmentVariable("OPENPIXELS_LANGUAGE") is { } forced && Known(forced)) return forced;
        if (Settings.Load().GetValueOrDefault("language") is { } saved && Known(saved)) return saved;
        return Match(CultureInfo.CurrentUICulture.Name) ?? Fallback;
    }

    private static Dictionary<string, string>? Load(string code)
    {
        if (code == "zh-CN") return null;
        using var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream($"OpenPixels.i18n.{code}.json");
        return stream is null ? null : JsonSerializer.Deserialize<Dictionary<string, string>>(stream);
    }

    public static void Initialize(string? language = null)
    {
        lock (Gate)
        {
            if (_language is not null && language is null) return;
            _language = language ?? Choose();
            _table = Load(_language);
            _fallback = _language is "zh-CN" or Fallback ? null : Load(Fallback);
        }
    }

    /// <summary>Saves the language to use from the next start.</summary>
    public static void Choose(string code)
    {
        var settings = Settings.Load();
        settings["language"] = code;
        Settings.Save(settings);
    }

    /// <summary>Text in the interface language.</summary>
    public static string T(string text)
    {
        Initialize();
        return _table?.GetValueOrDefault(text) ?? _fallback?.GetValueOrDefault(text) ?? text;
    }

    [GeneratedRegex(@"\{(\w+)\}")]
    private static partial Regex Placeholder();

    /// <summary>Text with {name} placeholders filled in, as in the web edition's t().</summary>
    public static string T(string text, params (string Name, object? Value)[] values)
    {
        var translated = T(text);
        return Placeholder().Replace(translated, match =>
            values.FirstOrDefault(value => value.Name == match.Groups[1].Value) is { Name: not null } found ? Convert.ToString(found.Value, CultureInfo.CurrentCulture) ?? "" : match.Value);
    }
}

/// <summary>Translated text in XAML: Text="{local:Tr 打开照片}" or {local:Tr '带空格 的文字'}.</summary>
public sealed class TrExtension(string text) : MarkupExtension
{
    public string Text { get; } = text;

    public override object ProvideValue(IServiceProvider serviceProvider) => I18n.T(Text);
}

/// <summary>Sizes that depend on the interface language, for XAML via x:Static.</summary>
public static class LanguageLayout
{
    private static bool Chinese => I18n.Language.StartsWith("zh", StringComparison.Ordinal);

    /// <summary>The column for slider and field names; names run longer outside Chinese.</summary>
    public static double LabelWidth => Chinese ? 84 : 104;

    public static Avalonia.Controls.GridLength LabelColumn => new(LabelWidth);
}
