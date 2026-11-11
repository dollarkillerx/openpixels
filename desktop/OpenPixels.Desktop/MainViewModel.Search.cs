using System.Collections.ObjectModel;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;

namespace OpenPixels.Desktop;

/// <summary>Searching the library by what photos show, and the tags found the same way.</summary>
public sealed partial class MainViewModel
{
    private readonly Lazy<ImageSearch> _search = new(() => new ImageSearch());
    private readonly JsonFile<Dictionary<string, string>> _embeddingFile = new("search-siglip2.json");
    private Dictionary<string, float[]>? _embeddings;
    private readonly DispatcherTimer _searchTimer = new() { Interval = TimeSpan.FromMilliseconds(350) };
    /// <summary>Each indexed photo's similarity to the query, while one is typed; null shows all.</summary>
    private Dictionary<string, double>? _searchScores;
    /// <summary>The score a photo needs to show: the match threshold, or the best score if none reach it.</summary>
    private double _searchCut;
    /// <summary>The tag each entry of <see cref="TagFilters"/> stands for; null is "all".</summary>
    private readonly List<string?> _tagFilterKeys = [null];
    private readonly Dictionary<string, IReadOnlyList<PhotoTag>> _photoTags = [];
    private bool _searchHooked;

    public bool SearchAvailable => ImageSearch.Available;

    public ObservableCollection<string> TagFilters { get; } = [I18n.T("全部标签")];

    [ObservableProperty]
    private int _tagFilterIndex;

    [ObservableProperty]
    private string _searchText = "";

    private Dictionary<string, float[]> Embeddings => _embeddings ??= _embeddingFile.Load()
        .Select(pair => (pair.Key, Value: ImageSearch.Unpack(pair.Value)))
        .Where(pair => pair.Value is not null)
        .ToDictionary(pair => pair.Key, pair => pair.Value!);

    private bool NeedsEmbedding(PhotoItem photo) => SearchAvailable && !Embeddings.ContainsKey(photo.Id);

    private void SaveEmbeddings() => _embeddingFile.Save(Embeddings.ToDictionary(pair => pair.Key, pair => ImageSearch.Pack(pair.Value)));

    partial void OnTagFilterIndexChanged(int value) => RefreshVisible();

    partial void OnSearchTextChanged(string value)
    {
        if (!_searchHooked)
        {
            _searchHooked = true;
            _searchTimer.Tick += async (_, _) =>
            {
                _searchTimer.Stop();
                await ApplySearchAsync();
            };
        }
        _searchTimer.Stop();
        _searchTimer.Start();
    }

    /// <summary>Whether a photo passes the search and tag filters.</summary>
    private bool PassesSearch(PhotoItem photo)
    {
        if (_searchScores is not null && _searchScores.GetValueOrDefault(photo.Id, double.MinValue) < _searchCut) return false;
        var key = TagFilterIndex > 0 && TagFilterIndex < _tagFilterKeys.Count ? _tagFilterKeys[TagFilterIndex] : null;
        return key is null || (_photoTags.TryGetValue(photo.Id, out var tags) && tags.Any(tag => tag.Key == key));
    }

    /// <summary>While searching, the best matches come first.</summary>
    private IEnumerable<PhotoItem> SearchOrder(IEnumerable<PhotoItem> photos) =>
        _searchScores is { } scores ? photos.OrderByDescending(photo => scores.GetValueOrDefault(photo.Id)) : photos;

    /// <summary>Scores every indexed photo against the query, indexing the rest first.</summary>
    public async Task ApplySearchAsync()
    {
        var query = SearchText.Trim();
        if (query.Length == 0 || !SearchAvailable)
        {
            _searchScores = null;
            RefreshVisible();
            return;
        }
        if (Photos.Any(NeedsEmbedding) && !IsBusy) await Analyze();
        try
        {
            var text = await Task.Run(() => _search.Value.EmbedText(query));
            if (query != SearchText.Trim()) return;
            _searchScores = Photos.Where(photo => Embeddings.ContainsKey(photo.Id))
                .ToDictionary(photo => photo.Id, photo => ImageSearch.Similarity(Embeddings[photo.Id], text));
            // The closest photo always shows, so a query phrased a little differently still finds something.
            _searchCut = Math.Min(ImageSearch.MatchThreshold, _searchScores.Count > 0 ? _searchScores.Values.Max() : 0);
            RefreshVisible();
            var found = _searchScores.Count(pair => pair.Value >= ImageSearch.MatchThreshold);
            LibraryStatus = _searchScores.Count == 0 ? I18n.T("还没有可搜索的照片")
                : found == 0 ? I18n.T("没有明显相符的照片，显示最接近“{query}”的一张", ("query", query))
                : I18n.T("找到 {count} 张与“{query}”相符的照片，最相符的在前", ("count", found), ("query", query));
        }
        catch (Exception exception)
        {
            LibraryStatus = I18n.T("搜索失败：{exception}", ("exception", Messages.Friendly(exception)));
        }
    }

    /// <summary>Gives each indexed photo its tags, and lists the tags in use with their counts.</summary>
    private async Task RefreshTagsAsync()
    {
        if (!SearchAvailable) return;
        var indexed = Photos.Where(photo => Embeddings.ContainsKey(photo.Id)).ToList();
        var found = await Task.Run(() => indexed.ToDictionary(photo => photo.Id, photo => _search.Value.TagsOf(Embeddings[photo.Id])));
        _photoTags.Clear();
        foreach (var (id, tags) in found) _photoTags[id] = tags;
        foreach (var photo in Photos) photo.TagText = _photoTags.TryGetValue(photo.Id, out var tags) ? string.Join(" · ", tags.Select(tag => tag.Name)) : "";
        var chosen = TagFilterIndex > 0 && TagFilterIndex < _tagFilterKeys.Count ? _tagFilterKeys[TagFilterIndex] : null;
        var counts = ImageSearch.Tags
            .Select(tag => (Tag: tag, Count: _photoTags.Values.Count(tags => tags.Contains(tag))))
            .Where(entry => entry.Count > 0)
            .ToList();
        _tagFilterKeys.RemoveRange(1, _tagFilterKeys.Count - 1);
        _tagFilterKeys.AddRange(counts.Select(entry => (string?)entry.Tag.Key));
        string[] names = [I18n.T("全部标签"), .. counts.Select(entry => I18n.T("{tag}（{count}）", ("tag", entry.Tag.Name), ("count", entry.Count)))];
        // Updated in place, as clearing the list would reset its selection.
        for (var index = 0; index < names.Length; index++)
        {
            if (index < TagFilters.Count && TagFilters[index] != names[index]) TagFilters[index] = names[index];
            else if (index >= TagFilters.Count) TagFilters.Add(names[index]);
        }
        while (TagFilters.Count > names.Length) TagFilters.RemoveAt(TagFilters.Count - 1);
        TagFilterIndex = Math.Max(0, _tagFilterKeys.IndexOf(chosen));
        RefreshVisible();
    }
}
