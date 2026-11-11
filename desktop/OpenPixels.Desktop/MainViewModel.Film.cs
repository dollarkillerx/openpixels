using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;

namespace OpenPixels.Desktop;

/// <summary>Film simulation: with a film look chosen, its print, format and darkroom settings.</summary>
public sealed partial class MainViewModel
{
    public SliderGroup FilmGroup { get; private set; } = null!;

    /// <summary>The print choices, the first being the film's own pairing.</summary>
    public ObservableCollection<FilmPrint> PrintChoices { get; } = [];

    public IReadOnlyList<string> FormatChoices { get; } = FilmSim.FormatNames;

    [ObservableProperty]
    private bool _filmActive;

    /// <summary>A slide is projected, so it has no print or printer lights.</summary>
    [ObservableProperty]
    private bool _filmPrinted;

    [ObservableProperty]
    private FilmPrint? _selectedPrint;

    [ObservableProperty]
    private int _selectedFormat = FilmSim.DefaultFormat;

    private void MakeFilmSliders() => FilmGroup = new SliderGroup(Edits.Film.Title, Edits.Film.Sliders.Select(Make).ToList());

    /// <summary>Shows the film settings of the current edit. Called while showing an edit.</summary>
    private void ShowFilm()
    {
        var film = FilmSim.Film((int)_edit.GetValueOrDefault("lutId"));
        FilmActive = film is not null;
        FilmPrinted = film is { Slide: false };
        PrintChoices.Clear();
        if (film is not null && !film.Slide)
        {
            var own = FilmSim.Print(film.Print);
            if (own is not null) PrintChoices.Add(new FilmPrint(0, own.Kind, I18n.T("默认：{name}", ("name", own.Name))));
            foreach (var print in FilmSim.Prints) PrintChoices.Add(print);
        }
        var chosen = (int)_edit.GetValueOrDefault("filmPrint");
        SelectedPrint = PrintChoices.FirstOrDefault(print => print.Id == chosen) ?? PrintChoices.FirstOrDefault();
        SelectedFormat = FilmSim.FormatIndex(_edit);
        // Printer lights mean nothing to a slide.
        foreach (var slider in FilmGroup.Sliders.Where(slider => slider.Spec.Key.StartsWith("filmLight", StringComparison.Ordinal))) slider.IsVisible = film is not { Slide: true };
    }

    partial void OnSelectedPrintChanged(FilmPrint? value)
    {
        if (_showingEdit || value is null || !_imageOpen) return;
        ApplyEdit(With(("filmPrint", value.Id, 0)));
    }

    partial void OnSelectedFormatChanged(int value)
    {
        if (_showingEdit || !_imageOpen || value < 0 || value >= FilmSim.FormatsMm.Length) return;
        ApplyEdit(With(("filmFormat", value == FilmSim.DefaultFormat ? 0 : FilmSim.FormatsMm[value], 0)));
    }
}
